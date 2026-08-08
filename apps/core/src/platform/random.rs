use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::shared::error::{AppError, ErrorCode};

/// 编码到每个随机 Bearer/CSRF 令牌中的熵字节数。
pub const TOKEN_BYTES: usize = 32;

/// 生成 256 个随机位，并以无填充的 URL 安全 base64 返回。
///
/// # Errors
///
/// 操作系统随机源不可用时返回 [`AppError`]。
pub fn random_token() -> Result<String, AppError> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[must_use]
/// 计算 `value` 的原始 32 字节 SHA-256 摘要。
pub fn sha256(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

#[must_use]
/// 计算按 identity-v1 前缀和调用方域分隔的 SHA-256 摘要。
///
/// 摘要输入为 `mediaflow.identity.v1`、一个 NUL 分隔符、`domain`、另一个 NUL 和 `value`，
/// 从而避免候选哈希与源哈希可以互换。
pub fn domain_digest(domain: &[u8], value: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"mediaflow.identity.v1\0");
    digest.update(domain);
    digest.update([0]);
    digest.update(value);
    digest.finalize().into()
}
