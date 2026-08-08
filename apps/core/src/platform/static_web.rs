use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::http::{Response, StatusCode, header};

use crate::shared::error::{AppError, ErrorCode};

#[must_use]
/// 从请求路径去除前导斜杠，并将其拼接到绝对或相对 `root`。
///
/// 仅当每个组件都是普通组件或 `.` 时才接受去除前导斜杠后的请求；父级遍历、根组件和平台前缀会返回 `None`。
/// 这是词法检查，因此提供文件的调用方还必须规范化结果，并强制其位于 `root` 之内。
pub fn safe_asset_path(root: &Path, request_path: &str) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in Path::new(request_path.trim_start_matches('/')).components() {
        match component {
            Component::Normal(value) => relative.push(value),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(root.join(relative))
}

/// 读取安全的静态资源，或为非 API 客户端路由读取 SPA 索引。
///
/// # Errors
///
/// 遍历、符号链接逃逸、I/O 或响应构造失败时返回 [`AppError`]。
pub async fn static_or_spa_response(
    root: &Path,
    request_path: &str,
) -> Result<Option<Response<Body>>, AppError> {
    if !root.is_dir() {
        return Ok(None);
    }
    let canonical_root = tokio::fs::canonicalize(root).await.map_err(static_error)?;
    let requested = safe_asset_path(root, request_path)
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "invalid static resource path"))?;

    if requested.is_file() {
        let canonical_requested = tokio::fs::canonicalize(&requested)
            .await
            .map_err(static_error)?;
        if !canonical_requested.starts_with(&canonical_root) {
            return Err(AppError::new(
                ErrorCode::NotFound,
                "static resource escapes distribution root",
            ));
        }
        let cache_policy = if is_fingerprinted_asset_request(request_path) {
            CachePolicy::Immutable
        } else {
            CachePolicy::NoStore
        };
        return read_response(&canonical_requested, cache_policy)
            .await
            .map(Some);
    }

    if is_explicit_asset_request(request_path) {
        return Ok(None);
    }

    let index = canonical_root.join("index.html");
    if !index.is_file() {
        return Ok(None);
    }
    let canonical_index = tokio::fs::canonicalize(&index)
        .await
        .map_err(static_error)?;
    if !canonical_index.starts_with(&canonical_root) {
        return Err(AppError::new(
            ErrorCode::NotFound,
            "SPA index escapes distribution root",
        ));
    }
    read_response(&canonical_index, CachePolicy::NoStore)
        .await
        .map(Some)
}

#[derive(Clone, Copy)]
enum CachePolicy {
    Immutable,
    NoStore,
}

impl CachePolicy {
    const fn header(self) -> &'static str {
        match self {
            Self::Immutable => "public, max-age=31536000, immutable",
            Self::NoStore => "no-store",
        }
    }
}

fn is_explicit_asset_request(request_path: &str) -> bool {
    let path = Path::new(request_path.trim_start_matches('/'));
    path.components()
        .next()
        .is_some_and(|component| component.as_os_str() == "assets")
        || path.extension().is_some()
}

fn is_fingerprinted_asset_request(request_path: &str) -> bool {
    let path = Path::new(request_path.trim_start_matches('/'));
    if path
        .components()
        .next()
        .is_none_or(|component| component.as_os_str() != "assets")
    {
        return false;
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit_once('-'))
        .is_some_and(|(_, fingerprint)| {
            let bytes = fingerprint.as_bytes();
            bytes.len() == 8
                && bytes
                    .iter()
                    .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'_' | b'-'))
                && bytes.iter().any(u8::is_ascii_lowercase)
                && bytes.iter().any(u8::is_ascii_uppercase)
                && bytes
                    .iter()
                    .any(|value| value.is_ascii_digit() || matches!(value, b'_' | b'-'))
        })
}

async fn read_response(path: &Path, cache_policy: CachePolicy) -> Result<Response<Body>, AppError> {
    let bytes = tokio::fs::read(path).await.map_err(static_error)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(path))
        .header(header::CACHE_CONTROL, cache_policy.header())
        .body(Body::from(bytes))
        .map_err(static_error)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("avif") => "image/avif",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("webp") => "image/webp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn static_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{StatusCode, header};

    use super::static_or_spa_response;

    #[tokio::test]
    async fn missing_asset_is_not_rewritten_to_the_spa_index() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), "spa-index").unwrap();

        let response = static_or_spa_response(root.path(), "/assets/missing.js")
            .await
            .unwrap();

        assert!(response.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spa_index_symlink_cannot_escape_the_distribution_root() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("index.html"), "outside-secret").unwrap();
        symlink(
            outside.path().join("index.html"),
            root.path().join("index.html"),
        )
        .unwrap();

        let error = static_or_spa_response(root.path(), "/tasks")
            .await
            .expect_err("an escaping SPA index must be rejected");

        assert_eq!(error.code(), crate::shared::error::ErrorCode::NotFound);
        assert!(!error.safe_message().contains("outside-secret"));
    }

    #[tokio::test]
    async fn serves_vite_asset_with_content_type_and_immutable_cache_policy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("assets")).unwrap();
        std::fs::write(root.path().join("index.html"), "spa-index").unwrap();
        std::fs::write(root.path().join("assets/index-BSfuXy_e.js"), "asset-body").unwrap();

        let response = static_or_spa_response(root.path(), "/assets/index-BSfuXy_e.js")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"asset-body");
    }

    #[tokio::test]
    async fn spa_index_is_never_cached() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), "spa-index").unwrap();

        let response = static_or_spa_response(root.path(), "/tasks")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }

    #[tokio::test]
    async fn direct_index_request_is_never_cached() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), "spa-index").unwrap();

        let response = static_or_spa_response(root.path(), "/index.html")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }

    #[tokio::test]
    async fn traversal_is_rejected_before_any_file_read() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), "spa-index").unwrap();

        let error = static_or_spa_response(root.path(), "/../outside.txt")
            .await
            .expect_err("traversal must be rejected");

        assert_eq!(error.code(), crate::shared::error::ErrorCode::NotFound);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn asset_symlink_cannot_escape_the_distribution_root() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("assets")).unwrap();
        std::fs::write(outside.path().join("app-abc123.js"), "outside-secret").unwrap();
        symlink(
            outside.path().join("app-abc123.js"),
            root.path().join("assets/app-abc123.js"),
        )
        .unwrap();

        let error = static_or_spa_response(root.path(), "/assets/app-abc123.js")
            .await
            .expect_err("an escaping asset symlink must be rejected");

        assert_eq!(error.code(), crate::shared::error::ErrorCode::NotFound);
        assert!(!error.safe_message().contains("outside-secret"));
    }

    #[tokio::test]
    async fn stable_root_file_is_not_immutable_cached() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("favicon.ico"), "icon").unwrap();

        let response = static_or_spa_response(root.path(), "/favicon.ico")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }

    #[tokio::test]
    async fn non_fingerprinted_asset_is_not_immutable_cached() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("assets")).unwrap();
        std::fs::write(root.path().join("assets/app.js"), "asset").unwrap();

        let response = static_or_spa_response(root.path(), "/assets/app.js")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }

    #[tokio::test]
    async fn stable_semantic_asset_name_is_not_immutable_cached() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("assets")).unwrap();
        std::fs::write(root.path().join("assets/font-regular.woff2"), "font-body").unwrap();

        let response = static_or_spa_response(root.path(), "/assets/font-regular.woff2")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }
}
