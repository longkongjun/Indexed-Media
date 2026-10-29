//! Shared public-only outbound destination policy, independent of business adapters.
use async_trait::async_trait;
use std::net::{IpAddr, SocketAddr};
use url::{Host, Url};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetError {
    Rejected,
    Unavailable,
}

pub(crate) fn public_https_url(target: &str, max_len: usize) -> Option<Url> {
    let url = Url::parse(target).ok()?;
    if target.len() > max_len
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
        || url.port() == Some(0)
    {
        return None;
    }
    match url.host()? {
        Host::Ipv4(ip) if !public_ip(IpAddr::V4(ip)) => return None,
        Host::Ipv6(ip) if !public_ip(IpAddr::V6(ip)) => return None,
        Host::Domain(host) if host.eq_ignore_ascii_case("localhost") || !host.contains('.') => {
            return None;
        }
        _ => {}
    }
    Some(url)
}

pub(crate) fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || a == 0
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192
                    && ((b == 0 && c == 0)
                        || (b == 31 && c == 196)
                        || (b == 52 && c == 193)
                        || (b == 88 && c == 99)
                        || (b == 175 && c == 48)))
                || (a == 198 && (b == 18 || b == 19)))
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            // IANA special-purpose registries reviewed 2026-10-29. Conservative public
            // unicast excludes protocol assignments (including their reachable exceptions),
            // transition/relay, documentation and service prefixes, not just local addresses.
            (segments[0] & 0xe000) == 0x2000
                && !(segments[0] == 0x2001 && (segments[1] < 0x0200 || segments[1] == 0x0db8))
                && segments[0] != 0x2002
                && segments[0] != 0x3fff
                && !(segments[0] == 0x2620 && segments[1] == 0x004f && segments[2] == 0x8000)
        }
    }
}

pub(crate) fn valid_addresses(addresses: &[SocketAddr]) -> bool {
    !addresses.is_empty() && addresses.len() <= 16 && addresses.iter().all(|a| public_ip(a.ip()))
}

#[async_trait]
trait HostResolver: Send + Sync {
    async fn lookup(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, TargetError>;
}
struct SystemResolver;
#[async_trait]
impl HostResolver for SystemResolver {
    async fn lookup(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, TargetError> {
        tokio::net::lookup_host((host, port))
            .await
            .map(|addresses| addresses.take(17).collect())
            .map_err(|_| TargetError::Unavailable)
    }
}

pub(crate) async fn resolve_public(url: &Url) -> Result<Vec<SocketAddr>, TargetError> {
    resolve_with(url, &SystemResolver).await
}

async fn resolve_with(
    url: &Url,
    resolver: &dyn HostResolver,
) -> Result<Vec<SocketAddr>, TargetError> {
    public_https_url(url.as_str(), usize::MAX).ok_or(TargetError::Rejected)?;
    let port = url.port_or_known_default().ok_or(TargetError::Rejected)?;
    let addresses = match url.host().ok_or(TargetError::Rejected)? {
        Host::Ipv4(ip) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        Host::Ipv6(ip) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        Host::Domain(host) => resolver.lookup(host, port).await?,
    };
    if !valid_addresses(&addresses) {
        return Err(TargetError::Rejected);
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Resolver {
        addresses: Vec<SocketAddr>,
        calls: AtomicUsize,
    }
    #[async_trait]
    impl HostResolver for Resolver {
        async fn lookup(&self, _: &str, _: u16) -> Result<Vec<SocketAddr>, TargetError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.addresses.clone())
        }
    }
    #[tokio::test]
    async fn validates_every_dns_answer_and_returns_one_checked_resolution() {
        let public: SocketAddr = "8.8.8.8:443".parse().unwrap();
        let private: SocketAddr = "10.0.0.1:443".parse().unwrap();
        let url = public_https_url("https://feed.example.test/path", 4096).unwrap();
        for addresses in [vec![], vec![public, private], vec![public; 17]] {
            let resolver = Resolver {
                addresses,
                calls: AtomicUsize::new(0),
            };
            assert_eq!(
                resolve_with(&url, &resolver).await,
                Err(TargetError::Rejected)
            );
            assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        }
        let resolver = Resolver {
            addresses: vec![public],
            calls: AtomicUsize::new(0),
        };
        assert_eq!(resolve_with(&url, &resolver).await.unwrap(), vec![public]);
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn url_policy_rejects_nonpublic_literals_and_accepts_https_queries() {
        for url in [
            "https://127.0.0.1/",
            "https://10.0.0.1/",
            "https://169.254.169.254/",
            "https://[::1]/",
            "https://[::ffff:8.8.8.8]/",
            "https://[fc00::1]/",
            "https://localhost/",
            "https://internal/",
            "http://example.com/",
            "https://a:b@example.com/",
            "https://example.com/#x",
            "https://example.com:0/",
        ] {
            assert!(public_https_url(url, 4096).is_none(), "{url}");
        }
        assert!(public_https_url("https://example.com/feed?opaque=secret", 4096).is_some());
    }
}
