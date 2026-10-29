//! Production feed destinations must be rejected before contacting a private endpoint.
use mediaflow_core::automation::rss::client::{FeedClient, FeedClientError, FeedHttpClient};
use mediaflow_core::platform::secrets::SecretBytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test]
async fn production_rejects_loopback_https_before_any_connection() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let connected = Arc::new(AtomicBool::new(false));
    let observed = connected.clone();
    let server = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            observed.store(true, Ordering::SeqCst);
            drop(stream);
        }
    });
    let result = FeedHttpClient::production()
        .unwrap()
        .fetch(
            &SecretBytes::new(format!("https://{address}/owned-test-feed").into_bytes()),
            None,
        )
        .await;
    server.abort();
    assert_eq!(
        (result, connected.load(Ordering::SeqCst)),
        (Err(FeedClientError::InvalidResponse), false),
        "policy must precede TCP/TLS"
    );
}
