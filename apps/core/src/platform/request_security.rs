use crate::identity::IdentityUseCases;
use crate::identity::model::AuthenticatedSession;
use crate::identity::service::IdentityService;
use crate::platform::random;
use crate::shared::error::{AppError, ErrorCode};
use axum::http::{HeaderMap, header};
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};
use subtle::ConstantTimeEq;
use url::Url;

/// 保存原始管理员会话令牌的安全仅主机 Cookie 名称。
pub const SESSION_COOKIE_NAME: &str = "__Host-mediaflow_session";
/// 用于管理员会话 Cookie 的无状态提取器/认证器。
pub struct SessionGuard;
/// 用于绑定会话 CSRF 响应头的无状态常数时间验证器。
pub struct CsrfGuard;

impl SessionGuard {
    /// 认证会话 Cookie 并滚动延长其空闲过期时间。
    ///
    /// # Errors
    ///
    /// Cookie 缺失或会话无效时返回 [`ErrorCode::SessionExpired`]；数据库失败会按其 [`AppError`] 返回。
    pub async fn authenticate(
        headers: &HeaderMap,
        service: &IdentityService,
    ) -> Result<AuthenticatedSession, AppError> {
        let token = session_token(headers)
            .ok_or_else(|| AppError::new(ErrorCode::SessionExpired, "session cookie missing"))?;
        service.authenticate(token).await
    }

    /// 认证会话 Cookie，但不改变空闲过期时间或最近使用时间。
    ///
    /// # Errors
    ///
    /// Cookie 缺失或会话无效时返回 [`ErrorCode::SessionExpired`]；数据库失败会按其 [`AppError`] 返回。
    pub async fn authenticate_without_sliding(
        headers: &HeaderMap,
        service: &IdentityService,
    ) -> Result<AuthenticatedSession, AppError> {
        let token = session_token(headers)
            .ok_or_else(|| AppError::new(ErrorCode::SessionExpired, "session cookie missing"))?;
        service.authenticate_without_sliding(token).await
    }
}
impl CsrfGuard {
    /// 以常数时间比较 `x-csrf-token` 与已认证会话摘要。
    ///
    /// # Errors
    ///
    /// 响应头缺失、非文本或不匹配时返回 [`ErrorCode::CsrfInvalid`]。
    pub fn validate(headers: &HeaderMap, session: &AuthenticatedSession) -> Result<(), AppError> {
        let supplied = headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| AppError::new(ErrorCode::CsrfInvalid, "csrf header missing"))?;
        if random::sha256(supplied.as_bytes())
            .ct_eq(&session.csrf_sha256)
            .unwrap_u8()
            == 1
        {
            Ok(())
        } else {
            Err(AppError::new(
                ErrorCode::CsrfInvalid,
                "csrf digest mismatch",
            ))
        }
    }
}
/// 要求使用 Origin/Fetch-Metadata/Referer 提供针对 `expected` 的同源浏览器证明。
///
/// Origin 组件按协议、主机和有效端口比较。若存在 `Sec-Fetch-Site`，其必须为 `same-origin`；
/// 否则需要 Origin 或 Referer 证明。
///
/// # Errors
///
/// 证明响应头缺失、格式错误、不透明（`null`）、跨域或互相矛盾时返回 [`ErrorCode::OriginUntrusted`]。
pub fn validate_same_origin(headers: &HeaderMap, expected: &Url) -> Result<(), AppError> {
    if let Some(supplied) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        if supplied == "null" {
            return Err(untrusted_origin());
        }
        let parsed = Url::parse(supplied).map_err(|_| untrusted_origin())?;
        if !same_origin(&parsed, expected) {
            return Err(untrusted_origin());
        }
    }
    match headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
    {
        Some("same-origin") => return Ok(()),
        Some(_) => return Err(untrusted_origin()),
        None => {}
    }
    let supplied = headers
        .get(header::ORIGIN)
        .or_else(|| headers.get(header::REFERER))
        .and_then(|value| value.to_str().ok())
        .ok_or_else(untrusted_origin)?;
    if supplied == "null" {
        return Err(untrusted_origin());
    }
    let parsed = Url::parse(supplied).map_err(|_| untrusted_origin())?;
    if same_origin(&parsed, expected) {
        Ok(())
    } else {
        Err(untrusted_origin())
    }
}
#[must_use]
/// 推导用作登录节流来源键的客户端 IP 字符串。
///
/// 仅在直接对等方受信任时才考虑 `X-Forwarded-For`。链会从代理一端剥离至第一个不受信任地址；
/// 格式错误或全受信任的链会回退到直接对等方，缺失对等方则为 `"unknown-peer"`。
pub fn source_key(headers: &HeaderMap, peer: Option<SocketAddr>, trusted: &[IpNet]) -> String {
    let peer_ip = peer.map(|address| address.ip());
    if peer_ip.is_some_and(|ip| trusted.iter().any(|network| network.contains(&ip)))
        && let Some(forwarded_chain) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
    {
        for hop in forwarded_chain.split(',').rev() {
            let Ok(address) = hop.trim().parse::<IpAddr>() else {
                return peer_ip.map_or_else(|| "unknown-peer".to_owned(), |ip| ip.to_string());
            };
            if !trusted.iter().any(|network| network.contains(&address)) {
                return address.to_string();
            }
        }
    }
    peer_ip.map_or_else(|| "unknown-peer".to_owned(), |ip| ip.to_string())
}
fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}
pub(crate) fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|item| {
            item.strip_prefix(SESSION_COOKIE_NAME)
                .and_then(|rest| rest.strip_prefix('='))
        })
}
fn untrusted_origin() -> AppError {
    AppError::new(ErrorCode::OriginUntrusted, "same-origin proof rejected")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origins_are_compared_by_components() {
        let expected = Url::parse("https://mediaflow.example.test").unwrap();
        assert!(same_origin(
            &Url::parse("https://mediaflow.example.test/path").unwrap(),
            &expected
        ));
        assert!(!same_origin(
            &Url::parse("https://mediaflow.example.test.evil").unwrap(),
            &expected
        ));
    }

    #[test]
    fn forwarded_client_is_used_only_for_a_trusted_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.7".parse().unwrap());
        let trusted = ["127.0.0.1/32".parse().unwrap()];
        assert_eq!(
            source_key(&headers, Some("127.0.0.1:1234".parse().unwrap()), &trusted),
            "203.0.113.7"
        );
        assert_eq!(
            source_key(&headers, Some("192.0.2.9:1234".parse().unwrap()), &trusted),
            "192.0.2.9"
        );
    }

    #[test]
    fn forwarded_chain_is_peeled_from_the_trusted_proxy_side() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "6.6.6.6, 198.51.100.23, 203.0.113.9, 127.0.0.2"
                .parse()
                .unwrap(),
        );
        let trusted = [
            "127.0.0.0/8".parse().unwrap(),
            "203.0.113.0/24".parse().unwrap(),
        ];
        assert_eq!(
            source_key(&headers, Some("127.0.0.1:1234".parse().unwrap()), &trusted),
            "198.51.100.23",
            "client-prepended left values must not override the first untrusted hop"
        );
    }

    #[test]
    fn all_trusted_or_invalid_forwarded_hops_fall_back_to_direct_peer() {
        let trusted = ["127.0.0.0/8".parse().unwrap()];
        for value in ["127.0.0.2, 127.0.0.3", "not-an-ip"] {
            let mut headers = HeaderMap::new();
            headers.insert("x-forwarded-for", value.parse().unwrap());
            assert_eq!(
                source_key(&headers, Some("127.0.0.1:1234".parse().unwrap()), &trusted),
                "127.0.0.1"
            );
        }
    }
}
