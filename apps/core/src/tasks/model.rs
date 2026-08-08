use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 扫描任务的持久化生命周期状态。
pub enum ScanStatus {
    /// 可供工作器认领。
    Queued,
    /// 由有效租约持有，并接收观测批次。
    Running,
    /// 带有一个或多个已记录条目错误的终态成功。
    PartialSuccess,
    /// 没有已记录条目错误的终态成功。
    Completed,
    /// 终态失败，包括无法恢复的根目录/能力错误。
    Failed,
    /// 收到取消请求后达到的终态。
    Cancelled,
}

impl ScanStatus {
    #[must_use]
    /// 返回此状态稳定的数据库/线缆拼写。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::PartialSuccess => "partial-success",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
/// 为现有扫描任务创建新尝试行的原因。
pub enum AttemptReason {
    /// 随任务创建的首次尝试。
    Initial,
    /// 用户显式重试了终态任务。
    ManualRetry,
    /// 运行时回收了过期的运行中租约。
    Recovery,
}

impl AttemptReason {
    #[must_use]
    /// 返回此原因稳定的数据库拼写。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::ManualRetry => "manual_retry",
            Self::Recovery => "recovery",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// 与当前任务尝试关联的计数器快照。
///
/// 成功的存储操作会返回其最新持久化快照。公开字段不强制单调性；发出公开进度事件的操作还要求每个计数
/// 均落在 JavaScript 安全整数范围内。
pub struct ScanCounts {
    /// 生产者报告的已枚举条目目录数量。
    pub visited_directories: u64,
    /// 存储生成的快照报告已作为观测持久化的常规文件。
    pub observed_files: u64,
    /// 生产者报告的不受支持或有意跳过的文件系统条目数量。
    pub skipped_entries: u64,
    /// 存储生成的快照报告持久化错误行，而不是它们累计的出现次数。
    pub errors: u64,
}

#[derive(Clone)]
/// 幂等扫描任务创建事务的账户范围输入。
///
/// `Debug` 会隐藏原始幂等键；存储仅持久化其 SHA-256 摘要。
pub struct NewScanTask {
    /// 拥有任务的管理员账户。
    pub account_id: Uuid,
    /// 要快照到扫描批次中的可用收件箱。
    pub inbox_directory_id: Uuid,
    /// 将此账户/操作/收件箱请求绑定到单一结果的调用方键。
    pub idempotency_key: String,
    /// 创建事务中所有行/事件使用的 UTC 微秒。
    pub now_us: i64,
}

impl fmt::Debug for NewScanTask {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewScanTask")
            .field("account_id", &self.account_id)
            .field("inbox_directory_id", &self.inbox_directory_id)
            .field("idempotency_key", &"[REDACTED]")
            .field("now_us", &self.now_us)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 对账户可见的扫描状态和最新持久化计数器快照。
pub struct ScanTaskView {
    /// 稳定的任务 UUID。
    pub id: Uuid,
    /// 任务的每次尝试都会扫描的收件箱。
    pub inbox_directory_id: Uuid,
    /// 当前持久化生命周期状态。
    pub status: ScanStatus,
    /// 工作是否作为租约恢复而非普通尝试排队/运行。
    pub recovering: bool,
    /// 最新已检查点的计数器。
    pub counts: ScanCounts,
}

#[derive(Clone, Debug)]
/// 授予一个工作器修改运行中任务权限的乐观租约。
///
/// 每次写入均须匹配任务 ID、所有者、版本、运行状态和未过期截止时间。成功写入会返回 `version` 已递增的租约
/// （并可能续期截止时间）。
pub struct ScanLease {
    /// 租约授权的任务快照。
    pub task: ScanTaskView,
    /// 接收去重文件观测的扫描批次。
    pub batch_id: Uuid,
    /// 接收错误和生命周期时间戳的当前尝试。
    pub attempt_id: Uuid,
    /// 存储在租约行中的工作器标识符。
    pub owner: String,
    /// 下一次变更所需的乐观并发版本。
    pub version: i64,
    /// 超过该 UTC 微秒截止时间后，此租约无法写入。
    pub expires_at_us: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 扫描观察到的、对账户可见且路径安全的文件投影。
pub struct DiscoveredFileView {
    /// 稳定的去重文件观测 UUID。
    pub id: Uuid,
    /// 相对于收件箱的有损 UTF-8 路径。
    pub relative_path: String,
    /// 最近观察到的字节长度。
    pub size_bytes: u64,
    /// RFC 3339 UTC 修改时间戳。
    pub modified_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 为任务当前尝试报告的去重条目错误。
pub struct ScanErrorView {
    /// 稳定的错误行 UUID。
    pub id: Uuid,
    /// 稳定的扫描器错误代码。
    pub code: String,
    /// 相对于收件箱的有损 UTF-8 路径。
    pub relative_path: String,
    /// 本次尝试中观察到相同范围代码/路径的次数。
    pub occurrences: u64,
}

#[cfg(test)]
mod tests {
    use super::NewScanTask;

    #[test]
    fn new_scan_task_debug_redacts_the_raw_idempotency_key() {
        let sentinel = "raw-idempotency-secret";
        let request = NewScanTask {
            account_id: uuid::Uuid::nil(),
            inbox_directory_id: uuid::Uuid::nil(),
            idempotency_key: sentinel.to_owned(),
            now_us: 1,
        };

        let debug = format!("{request:?}");
        assert!(!debug.contains(sentinel));
        assert!(debug.contains("[REDACTED]"));
    }
}
