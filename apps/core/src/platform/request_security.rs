use crate::identity::IdentityUseCases;
use crate::identity::model::{AuthenticatedSession, DeviceAccessMode, Principal};
use crate::identity::service::IdentityService;
use crate::platform::random;
use crate::shared::error::{AppError, ErrorCode};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};
use subtle::ConstantTimeEq;
use url::Url;

/// 保存原始管理员会话令牌的安全仅主机 Cookie 名称。
pub const SESSION_COOKIE_NAME: &str = "__Host-mediaflow_session";
/// 设备令牌 `Authorization` 方案前缀；移动端凭据通道（M5 第二批）。
pub const DEVICE_TOKEN_SCHEME: &str = "Bearer";
/// 用于管理员会话 Cookie 与设备令牌的无状态提取器/认证器。
pub struct SessionGuard;
/// 用于绑定会话 CSRF 响应头的无状态常数时间验证器。
pub struct CsrfGuard;

impl SessionGuard {
    /// 认证请求凭据并滚动延长会话空闲过期时间。
    ///
    /// 通道由凭据位置决定：会话 Cookie 优先；无 Cookie 时回退到
    /// `Authorization: Bearer` 设备令牌（无空闲语义，仅节流更新 `last_used_at`）。
    /// 两通道不可互换——会话令牌放入 Authorization 头查设备表无果，反之亦然。
    ///
    /// # Errors
    ///
    /// 两种凭据均缺失或对应凭据无效时返回 [`ErrorCode::SessionExpired`]；数据库失败会按其 [`AppError`] 返回。
    pub async fn authenticate(
        headers: &HeaderMap,
        service: &IdentityService,
    ) -> Result<AuthenticatedSession, AppError> {
        if let Some(token) = session_token(headers) {
            return service.authenticate(token).await;
        }
        if let Some(token) = bearer_token(headers) {
            return service.authenticate_device(token).await;
        }
        Err(AppError::new(
            ErrorCode::SessionExpired,
            "session cookie or device token missing",
        ))
    }

    /// 认证请求凭据，但不改变会话空闲过期时间或最近使用时间。
    ///
    /// 通道选择与 [`SessionGuard::authenticate`] 一致；设备通道本无空闲语义，
    /// 差异仅体现在会话通道。
    ///
    /// # Errors
    ///
    /// 两种凭据均缺失或对应凭据无效时返回 [`ErrorCode::SessionExpired`]；数据库失败会按其 [`AppError`] 返回。
    pub async fn authenticate_without_sliding(
        headers: &HeaderMap,
        service: &IdentityService,
    ) -> Result<AuthenticatedSession, AppError> {
        if let Some(token) = session_token(headers) {
            return service.authenticate_without_sliding(token).await;
        }
        if let Some(token) = bearer_token(headers) {
            return service.authenticate_device(token).await;
        }
        Err(AppError::new(
            ErrorCode::SessionExpired,
            "session cookie or device token missing",
        ))
    }
}
impl CsrfGuard {
    /// 以常数时间比较 `x-csrf-token` 与已认证会话摘要。
    ///
    /// 设备令牌通道按"令牌即写证明"豁免 CSRF（显式附加的 Bearer 令牌按构造免疫
    /// 环境凭据型 CSRF），主体无摘要时直接放行。
    ///
    /// # Errors
    ///
    /// 会话通道下响应头缺失、非文本或不匹配时返回 [`ErrorCode::CsrfInvalid`]。
    pub fn validate(headers: &HeaderMap, session: &AuthenticatedSession) -> Result<(), AppError> {
        let Some(expected) = session.csrf_sha256 else {
            return Ok(());
        };
        let supplied = headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| AppError::new(ErrorCode::CsrfInvalid, "csrf header missing"))?;
        if random::sha256(supplied.as_bytes())
            .ct_eq(&expected)
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
/// 携带 `Authorization: Bearer` 头的请求直接放行：浏览器跨站请求无法附加自定义
/// Authorization 头（触发 CORS 预检且非同源不通过），携带该头的请求按构造不可能是
/// cookie 环境凭据型 CSRF——即"设备令牌即写证明"豁免（M5 第二批）。
///
/// # Errors
///
/// 证明响应头缺失、格式错误、不透明（`null`）、跨域或互相矛盾时返回 [`ErrorCode::OriginUntrusted`]。
pub fn validate_same_origin(headers: &HeaderMap, expected: &Url) -> Result<(), AppError> {
    if bearer_token(headers).is_some() {
        return Ok(());
    }
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
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case(DEVICE_TOKEN_SCHEME) {
        return None;
    }
    let token = rest.trim();
    if token.is_empty() { None } else { Some(token) }
}
fn untrusted_origin() -> AppError {
    AppError::new(ErrorCode::OriginUntrusted, "same-origin proof rejected")
}

/// 在全部 API 路由之前执行只读设备允许列表；新路由默认拒绝。
pub async fn enforce_device_access(
    State(service): State<IdentityService>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path().starts_with("/api/") && bearer_token(request.headers()).is_some() {
        let authenticated =
            SessionGuard::authenticate_without_sliding(request.headers(), &service).await;
        let authenticated = match authenticated {
            Err(error)
                if error.code() == ErrorCode::SessionExpired
                    && session_token(request.headers()).is_some() =>
            {
                // 无效cookie不能让已知只读设备绕过限制；目标路由仍保留原cookie认证优先级。
                service
                    .authenticate_device(bearer_token(request.headers()).unwrap_or_default())
                    .await
            }
            other => other,
        };
        match authenticated {
            Ok(session) if session.access_mode == DeviceAccessMode::MediaReadOnly => {
                let Principal::Device { device_id } = session.principal else {
                    return AppError::new(
                        ErrorCode::PermissionDenied,
                        "read-only identity is not a device",
                    )
                    .into_response();
                };
                if !media_read_only_request(request.method(), request.uri().path(), device_id) {
                    return AppError::new(
                        ErrorCode::PermissionDenied,
                        "device is restricted to media consumption",
                    )
                    .into_response();
                }
            }
            Ok(_) => {}
            // 非设备Bearer可能属于独立capability凭据面，最终认证仍由目标路由执行。
            Err(error) if error.code() == ErrorCode::SessionExpired => {}
            Err(error) => return error.into_response(),
        }
    }
    next.run(request).await
}

fn media_read_only_request(method: &Method, path: &str, device_id: uuid::Uuid) -> bool {
    let segments: Vec<_> = path
        .strip_prefix("/api/v1/")
        .unwrap_or_default()
        .split('/')
        .collect();
    match (method, segments.as_slice()) {
        (&Method::GET, ["media-items" | "media-libraries"] | ["integrations", "jellyfin"])
        | (&Method::POST, ["devices", "current", "token-rotation"]) => true,
        (&Method::GET, ["media-items", id] | ["media-items", id, "artwork"]) => {
            uuid::Uuid::parse_str(id).is_ok()
        }
        (&Method::GET | &Method::DELETE, ["devices", id]) => {
            uuid::Uuid::parse_str(id) == Ok(device_id)
        }
        _ => false,
    }
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
