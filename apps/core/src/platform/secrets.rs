use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::Path;

use chacha20poly1305::aead::{Aead as _, KeyInit as _, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rustix::fs::{FileType, Mode, OFlags};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// 每个实例的 XChaCha20-Poly1305 密钥精确字节长度。
pub const INSTANCE_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;
const INSTANCE_KEY_FILE: &str = "instance.key";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
/// 可绑定加密配置 AAD 的内置集成类型。
pub enum IntegrationKind {
    /// The Movie Database 元数据提供方。
    Tmdb,
    /// qBittorrent `WebUI` API 下载器。
    Qbittorrent,
    /// Transmission RPC 下载器。
    Transmission,
    /// 下载任务中的 magnet 或私有 torrent URL。
    DownloadTaskSource,
    /// RSS or Atom feed URL used by an automation source.
    AutomationRss,
    /// Encrypted conditional request cursor for one RSS source.
    AutomationRssCursor,
    /// HMAC key generated for an inbound automation webhook.
    AutomationWebhook,
    /// Encrypted fixed-action payload of a durable automation event.
    AutomationEventPayload,
    /// Idempotent one-time webhook rotation response.
    AutomationWebhookRotationReceipt,
}

impl IntegrationKind {
    const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Tmdb => b"tmdb",
            Self::Qbittorrent => b"qbittorrent",
            Self::Transmission => b"transmission",
            Self::DownloadTaskSource => b"download-task-source",
            Self::AutomationRss => b"automation-rss",
            Self::AutomationRssCursor => b"automation-rss-cursor",
            Self::AutomationWebhook => b"automation-webhook",
            Self::AutomationEventPayload => b"automation-event-payload",
            Self::AutomationWebhookRotationReceipt => b"automation-webhook-rotation-receipt",
        }
    }
}

/// 不可克隆的每实例认证加密密钥。
pub struct InstanceKey([u8; INSTANCE_KEY_BYTES]);

impl std::fmt::Debug for InstanceKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InstanceKey([REDACTED])")
    }
}

impl Drop for InstanceKey {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

impl InstanceKey {
    /// 在不跟随最终符号链接的前提下打开已有的 `instance.key`，或恰好创建一次。
    ///
    /// 创建的文件恰为 32 个随机字节、模式为 `0600`，使用前会同步并重新打开以进行权威校验。
    /// 已有文件过短、过长、不是普通文件、存在链接或权限过宽时会被拒绝，且绝不替换。
    ///
    /// # Errors
    ///
    /// 目录/密钥边界、随机源、持久化写入或校验失败时，返回 [`SecretError`]。
    pub fn load_or_create(config_dir: &Path) -> Result<Self, SecretError> {
        let directory = rustix::fs::open(
            config_dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| SecretError::Filesystem)?;
        match rustix::fs::openat(
            &directory,
            INSTANCE_KEY_FILE,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => read_verified_key(fd),
            Err(Errno::NOENT) => create_key(&directory),
            Err(_) => Err(SecretError::Filesystem),
        }
    }
}

/// 防止加密配置在行或版本之间迁移的 AAD 字段。
pub struct SecretAad {
    integration_id: Uuid,
    kind: IntegrationKind,
    config_version: i64,
    schema_version: u16,
}

impl SecretAad {
    #[must_use]
    /// 创建不可变的 AAD 绑定。
    pub const fn new(
        integration_id: Uuid,
        kind: IntegrationKind,
        config_version: i64,
        schema_version: u16,
    ) -> Self {
        Self {
            integration_id,
            kind,
            config_version,
            schema_version,
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(64);
        encoded.extend_from_slice(b"mediaflow.integration-secret\0");
        encoded.extend_from_slice(self.integration_id.as_bytes());
        encoded.push(0);
        encoded.extend_from_slice(self.kind.as_bytes());
        encoded.push(0);
        encoded.extend_from_slice(&self.config_version.to_be_bytes());
        encoded.extend_from_slice(&self.schema_version.to_be_bytes());
        encoded
    }
}

impl std::fmt::Debug for SecretAad {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretAad")
            .field("integration_id", &self.integration_id)
            .field("kind", &self.kind)
            .field("config_version", &self.config_version)
            .field("schema_version", &self.schema_version)
            .finish()
    }
}

/// 所有权明文内容在释放时清零，且绝不通过 Debug 暴露。
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    #[must_use]
    /// 包装具有所有权的秘密字节。
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    #[must_use]
    /// 仅在显式的加密/提供方边界借用明文。
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretBytes([REDACTED])")
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// 存储在 `SQLite` 中的带版本 nonce 与认证密文。
pub struct SealedSecret {
    schema_version: u16,
    nonce: [u8; NONCE_BYTES],
    ciphertext: Vec<u8>,
}

impl SealedSecret {
    /// 从持久化列重建已校验的密封记录。
    ///
    /// # Errors
    ///
    /// 拒绝 schema version 为零及短于 Poly1305 标签的密文。
    pub fn from_parts(
        schema_version: u16,
        nonce: [u8; NONCE_BYTES],
        ciphertext: Vec<u8>,
    ) -> Result<Self, SecretError> {
        if schema_version == 0 || ciphertext.len() < TAG_BYTES {
            return Err(SecretError::InvalidRecord);
        }
        Ok(Self {
            schema_version,
            nonce,
            ciphertext,
        })
    }

    #[must_use]
    /// 返回密封记录的 schema version。
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    #[must_use]
    /// 返回公开的随机 nonce。
    pub const fn nonce(&self) -> &[u8; NONCE_BYTES] {
        &self.nonce
    }

    #[must_use]
    /// 返回认证后的密文字节。
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }
}

impl std::fmt::Debug for SealedSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedSecret")
            .field("schema_version", &self.schema_version)
            .field("nonce", &"[REDACTED]")
            .field("ciphertext", &"[REDACTED]")
            .finish()
    }
}

/// 集成凭据的认证加密边界。
pub trait SecretCipher: Send + Sync {
    /// 使用新的随机 nonce 与提供的 AAD 加密明文。
    ///
    /// # Errors
    ///
    /// 随机源、记录形状或认证加密失败时返回错误。
    fn seal(&self, aad: &SecretAad, plaintext: &[u8]) -> Result<SealedSecret, SecretError>;
    /// 使用完全相同的 AAD 验证并解密密封记录。
    ///
    /// # Errors
    ///
    /// 记录形状或认证失败（包括 AAD 不匹配）时返回错误。
    fn open(&self, aad: &SecretAad, sealed: &SealedSecret) -> Result<SecretBytes, SecretError>;
}

impl SecretCipher for InstanceKey {
    fn seal(&self, aad: &SecretAad, plaintext: &[u8]) -> Result<SealedSecret, SecretError> {
        if aad.schema_version == 0 {
            return Err(SecretError::InvalidRecord);
        }
        let mut nonce = [0_u8; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| SecretError::Random)?;
        let mut key = Key::from(self.0);
        let cipher = XChaCha20Poly1305::new(&key);
        key.fill(0);
        let nonce_array = XNonce::from(nonce);
        let aad_bytes = aad.encode();
        let ciphertext = cipher
            .encrypt(
                &nonce_array,
                Payload {
                    msg: plaintext,
                    aad: &aad_bytes,
                },
            )
            .map_err(|_| SecretError::Authentication)?;
        SealedSecret::from_parts(aad.schema_version, nonce, ciphertext)
    }

    fn open(&self, aad: &SecretAad, sealed: &SealedSecret) -> Result<SecretBytes, SecretError> {
        if sealed.schema_version != aad.schema_version {
            return Err(SecretError::Authentication);
        }
        let mut key = Key::from(self.0);
        let cipher = XChaCha20Poly1305::new(&key);
        key.fill(0);
        let nonce = XNonce::from(sealed.nonce);
        let aad_bytes = aad.encode();
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &sealed.ciphertext,
                    aad: &aad_bytes,
                },
            )
            .map(SecretBytes::new)
            .map_err(|_| SecretError::Authentication)
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
/// 秘密文件和认证加密的稳定失败分类。
pub enum SecretError {
    /// 密钥目录或文件不可用、存在链接、不是普通文件或并非私有。
    #[error("instance secret filesystem boundary failed")]
    Filesystem,
    /// 操作系统随机源失败。
    #[error("instance secret random source failed")]
    Random,
    /// 持久化密封记录具有不受支持的形状。
    #[error("sealed secret record is invalid")]
    InvalidRecord,
    /// 密文认证或 AAD 校验失败。
    #[error("sealed secret authentication failed")]
    Authentication,
}

fn create_key(directory: &rustix::fd::OwnedFd) -> Result<InstanceKey, SecretError> {
    let mut bytes = [0_u8; INSTANCE_KEY_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| SecretError::Random)?;
    let fd = match rustix::fs::openat(
        directory,
        INSTANCE_KEY_FILE,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(fd) => fd,
        Err(Errno::EXIST) => {
            bytes.fill(0);
            let existing = rustix::fs::openat(
                directory,
                INSTANCE_KEY_FILE,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| SecretError::Filesystem)?;
            return read_verified_key(existing);
        }
        Err(_) => {
            bytes.fill(0);
            return Err(SecretError::Filesystem);
        }
    };
    rustix::fs::fchmod(&fd, Mode::RUSR | Mode::WUSR).map_err(|_| SecretError::Filesystem)?;
    let mut file = File::from(fd);
    if file.write_all(&bytes).is_err() || file.sync_all().is_err() {
        bytes.fill(0);
        return Err(SecretError::Filesystem);
    }
    drop(file);
    rustix::fs::fsync(directory).map_err(|_| SecretError::Filesystem)?;
    bytes.fill(0);
    let reopened = rustix::fs::openat(
        directory,
        INSTANCE_KEY_FILE,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| SecretError::Filesystem)?;
    read_verified_key(reopened)
}

fn read_verified_key(fd: rustix::fd::OwnedFd) -> Result<InstanceKey, SecretError> {
    let stat = rustix::fs::fstat(&fd).map_err(|_| SecretError::Filesystem)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_mode & 0o777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(SecretError::Filesystem);
    }
    let mut bytes = Vec::with_capacity(INSTANCE_KEY_BYTES + 1);
    File::from(fd)
        .take((INSTANCE_KEY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| SecretError::Filesystem)?;
    if bytes.len() != INSTANCE_KEY_BYTES {
        bytes.fill(0);
        return Err(SecretError::InvalidRecord);
    }
    let mut key = [0_u8; INSTANCE_KEY_BYTES];
    key.copy_from_slice(&bytes);
    bytes.fill(0);
    Ok(InstanceKey(key))
}
