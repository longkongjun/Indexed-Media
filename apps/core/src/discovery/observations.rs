use std::fmt;

use async_trait::async_trait;
use uuid::Uuid;

use crate::shared::error::AppError;
use crate::tasks::model::ScanCounts;

#[derive(Clone, Eq, PartialEq)]
/// 扫描期间在一个收件箱下观测到的文件元数据。
///
/// 原始路径和身份字节是持久化键；显示路径仅用于 API。`Debug` 会遮蔽这三者，
/// 避免泄露主机内容名称或文件系统身份。
pub struct FileObservation {
    /// 产生此观测结果的开放能力所属收件箱。
    pub inbox_directory_id: Uuid,
    /// 精确的主机原生相对路径字节。
    pub relative_path_bytes: Vec<u8>,
    /// 适合面向用户输出的有损 UTF-8 相对路径。
    pub relative_path_display: String,
    /// 用于重命名/变更跟踪的确定性设备、inode 与挂载点快照。
    pub identity_snapshot: Vec<u8>,
    /// 观测到的文件长度（字节）。
    pub size_bytes: u64,
    /// 自 Unix 纪元起的修改时间纳秒数；扫描时饱和转换为 `i64`。
    pub modified_at_ns: i64,
}

impl fmt::Debug for FileObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileObservation")
            .field("inbox_directory_id", &self.inbox_directory_id)
            .field("relative_path_bytes", &"[REDACTED]")
            .field("relative_path_display", &"[REDACTED]")
            .field("identity_snapshot", &"[REDACTED]")
            .field("size_bytes", &self.size_bytes)
            .field("modified_at_ns", &self.modified_at_ns)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
/// 保留在扫描结果中而不会中止遍历的可恢复单项失败。
///
/// `Debug` 会遮蔽两种路径形式。
pub struct ScanEntryError {
    /// 稳定错误码，例如 `entry.symlink_forbidden`。
    pub code: &'static str,
    /// 用于对出现次数去重的聚合范围。
    pub scope: &'static str,
    /// 用作持久化键的精确主机原生路径字节。
    pub relative_path_bytes: Vec<u8>,
    /// 返回给调用方的有损 UTF-8 路径。
    pub relative_path_display: String,
}

impl fmt::Debug for ScanEntryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScanEntryError")
            .field("code", &self.code)
            .field("scope", &self.scope)
            .field("relative_path_bytes", &"[REDACTED]")
            .field("relative_path_display", &"[REDACTED]")
            .finish()
    }
}

#[async_trait]
/// 扫描器观测结果与聚合计数的持久化检查点边界。
pub trait ObservationSink: Send + Sync {
    /// 连同最新累计进度一起持久化文件/错误观测结果。
    ///
    /// 实现应让批次和进度原子可见；成功的空批次仍可检查点化遍历进度。
    ///
    /// # Errors
    ///
    /// 当任务租约/状态不再允许写入，或批次无法提交时返回 [`AppError`]。发生错误后扫描器停止。
    async fn write_batch(
        &self,
        files: Vec<FileObservation>,
        errors: Vec<ScanEntryError>,
        progress: ScanCounts,
    ) -> Result<(), AppError>;
}

#[cfg(test)]
mod tests {
    use super::{FileObservation, ScanEntryError};

    #[test]
    fn observation_debug_redacts_raw_and_display_paths_and_identity() {
        let sentinel_path = "private/raw/movie.mkv";
        let sentinel_identity = b"sensitive-file-identity".to_vec();
        let observation = FileObservation {
            inbox_directory_id: uuid::Uuid::nil(),
            relative_path_bytes: sentinel_path.as_bytes().to_vec(),
            relative_path_display: sentinel_path.to_owned(),
            identity_snapshot: sentinel_identity.clone(),
            size_bytes: 1,
            modified_at_ns: 1,
        };
        let entry_error = ScanEntryError {
            code: "entry.unavailable",
            scope: "entry",
            relative_path_bytes: sentinel_path.as_bytes().to_vec(),
            relative_path_display: sentinel_path.to_owned(),
        };

        for debug in [format!("{observation:?}"), format!("{entry_error:?}")] {
            assert!(!debug.contains(sentinel_path));
            assert!(!debug.contains("sensitive-file-identity"));
            assert!(debug.contains("[REDACTED]"));
        }
    }
}
