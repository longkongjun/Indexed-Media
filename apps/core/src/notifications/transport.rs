//! Restricted optional webhook transport: public HTTPS, pinned DNS, no proxy or redirects.
use std::net::SocketAddr;
use std::time::Duration;

use crate::platform::outbound_http::{TargetError, public_https_url, resolve_public};
use async_trait::async_trait;
use url::Url;

use super::delivery_store::{DeliveryOutcome, DeliveryPayload};

/// External boundary; implementations must preserve bounded, redacted delivery semantics.
#[async_trait]
pub trait WebhookTransport: Send + Sync {
    /// Deliver one fixed payload, reusing its stable receiver idempotency identity.
    async fn deliver(&self, target: &str, payload: &DeliveryPayload) -> DeliveryOutcome;
}

/// Production transport with a total DNS/connect/request budget.
#[derive(Clone, Copy, Default)]
pub struct PublicHttpsTransport;

/// Validate URL syntax and literal destinations without performing a network request.
#[must_use]
pub fn valid_target(target: &str) -> bool {
    target_url(target).is_some()
}

fn target_url(target: &str) -> Option<Url> {
    public_https_url(target, 2048)
}

#[async_trait]
impl WebhookTransport for PublicHttpsTransport {
    async fn deliver(&self, target: &str, payload: &DeliveryPayload) -> DeliveryOutcome {
        let Some(url) = target_url(target) else {
            return DeliveryOutcome::Rejected;
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            let addresses = match resolve_public(&url).await {
                Ok(addresses) => addresses,
                Err(TargetError::Rejected) => return DeliveryOutcome::Rejected,
                Err(TargetError::Unavailable) => return DeliveryOutcome::Retryable,
            };
            // Resolving explicitly to checked addresses prevents a second DNS lookup/rebinding.
            send_pinned(url, &addresses, payload).await
        })
        .await
        .unwrap_or(DeliveryOutcome::Retryable)
    }
}

async fn send_pinned(
    url: Url,
    addresses: &[SocketAddr],
    payload: &DeliveryPayload,
) -> DeliveryOutcome {
    let Some(host) = url.host_str() else {
        return DeliveryOutcome::Rejected;
    };
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .resolve_to_addrs(host, addresses)
        .build()
    else {
        return DeliveryOutcome::Retryable;
    };
    let Ok(response) = client
        .post(url)
        .header("Idempotency-Key", &payload.delivery_id)
        .json(payload)
        .send()
        .await
    else {
        return DeliveryOutcome::Retryable;
    };
    let status = response.status();
    if status.is_success() {
        DeliveryOutcome::Delivered
    } else if matches!(status.as_u16(), 401 | 403) {
        DeliveryOutcome::Unauthorized
    } else if status.as_u16() == 429 || status.is_server_error() {
        DeliveryOutcome::Retryable
    } else {
        DeliveryOutcome::Rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::outbound_http::{public_ip, valid_addresses};
    #[test]
    fn rejects_private_reserved_mapped_and_mixed_dns_destinations() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.169.254",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "240.1.2.3",
            "::1",
            "::ffff:8.8.8.8",
            "fc00::1",
            "2001:db8::1",
            "192.88.99.2",
            "192.31.196.1",
            "192.52.193.1",
            "192.175.48.1",
            "2002:7f00:1::1",
            "3fff::1",
            "2620:4f:8000::1",
            "64:ff9b::7f00:1",
            "100:0:0:1::1",
            "5f00::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        let public: SocketAddr = "8.8.8.8:443".parse().unwrap();
        assert!(valid_addresses(&[public]));
        assert!(!valid_addresses(&[public, "10.0.0.1:443".parse().unwrap()]));
        assert!(!valid_addresses(&[]));
        assert!(!valid_addresses(&[public; 17]));
    }
    #[test]
    fn rejects_unsafe_urls_without_network() {
        for target in [
            "http://example.com/",
            "https://user:secret@example.com/",
            "https://example.com/#fragment",
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://localhost/",
            "https://internal/",
        ] {
            assert!(!valid_target(target), "{target}");
        }
        assert!(valid_target("https://example.com/events"));
    }
    #[tokio::test]
    async fn pinned_transport_sends_stable_identity_and_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let receiver = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let size = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..size]).to_lowercase();
            assert!(request.contains("idempotency-key: test-delivery"));
            assert!(request.contains("host: unresolvable.example.test:"));
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let payload = DeliveryPayload {
            delivery_id: "test-delivery".to_owned(),
            kind: "system.notice".to_owned(),
            title: "Title".to_owned(),
            body: "Body".to_owned(),
            task_id: None,
        };
        // Unit-only direct invocation verifies actual HTTP behavior; production validates HTTPS first.
        assert_eq!(
            send_pinned(
                Url::parse(&format!(
                    "http://unresolvable.example.test:{}/events",
                    address.port()
                ))
                .unwrap(),
                &[address],
                &payload
            )
            .await,
            DeliveryOutcome::Rejected
        );
        receiver.await.unwrap();
    }
}
