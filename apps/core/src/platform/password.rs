use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::platform::random;
use crate::shared::error::{AppError, ErrorCode};

/// 以 KiB 计的 Argon2id 内存成本（64 MiB）。
pub const MEMORY_KIB: u32 = 65_536;
/// 用于哈希的 Argon2id 通道数。
pub const PARALLELISM: u32 = 1;
/// 可接受的最小 Argon2id 迭代次数。
pub const MIN_TIME_COST: u32 = 2;
/// 配置缺失或无效时使用的迭代次数。
pub const DEFAULT_TIME_COST: u32 = 2;
const DEFAULT_DUMMY_PHC: &str = "$argon2id$v=19$m=65536,t=2,p=1$iAbZ67M4BUvVEVjjvIlTaw$PcXCzHwXlHxZ1zniSxhm2sugvOnaB5N79YQYt6mun/A";
const DUMMY_PASSWORD: &str = "mediaflow dummy credential path";

#[must_use]
/// 解析配置的迭代次数；输入无效时使用默认值，并钳制到最小值。
pub fn configured_time_cost(value: Option<&str>) -> u32 {
    value
        .and_then(|raw| raw.parse::<u32>().ok())
        .map_or(DEFAULT_TIME_COST, |cost| cost.max(MIN_TIME_COST))
}

#[derive(Clone)]
/// 对阻塞式 Argon2id 哈希和验证操作提供有界异步外观。
///
/// 克隆实例共享并发信号量、操作计数器、选定时间成本和伪 PHC。
pub struct PasswordEngine {
    semaphore: Arc<Semaphore>,
    time_cost: u32,
    operations: Arc<PasswordOperationCounts>,
    dummy_phc: Arc<str>,
}

#[derive(Default)]
/// 提交给引擎的哈希/验证尝试共享进程计数器。
pub struct PasswordOperationCounts {
    hashes: AtomicUsize,
    verifications: AtomicUsize,
}

impl PasswordOperationCounts {
    #[must_use]
    /// 返回已开始的哈希调用数，包括后来失败的调用。
    pub fn hashes(&self) -> usize {
        self.hashes.load(Ordering::Relaxed)
    }
    #[must_use]
    /// 返回已开始的验证调用数，包括后来失败的调用。
    pub fn verifications(&self) -> usize {
        self.verifications.load(Ordering::Relaxed)
    }
}

impl PasswordEngine {
    #[must_use]
    /// 创建有界密码引擎及其流量前使用的伪凭据。
    ///
    /// 零 `max_concurrent` 会提升为一个工作器许可，`time_cost` 会钳制到 [`MIN_TIME_COST`]。
    ///
    /// # Panics
    ///
    /// 仅当编译期有效的 Argon2 参数无法生成伪 PHC 时发生 panic。
    pub fn new(max_concurrent: usize, time_cost: u32) -> Self {
        let time_cost = time_cost.max(MIN_TIME_COST);
        let dummy_phc = if time_cost == DEFAULT_TIME_COST {
            Arc::from(DEFAULT_DUMMY_PHC)
        } else {
            Arc::from(
                hash_blocking(DUMMY_PASSWORD, time_cost).expect("valid Argon2 dummy parameters"),
            )
        };
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            time_cost,
            operations: Arc::new(PasswordOperationCounts::default()),
            dummy_phc,
        }
    }

    #[must_use]
    /// 创建引擎并返回其操作计数器的共享句柄。
    pub fn new_counted(
        max_concurrent: usize,
        time_cost: u32,
    ) -> (Self, Arc<PasswordOperationCounts>) {
        let engine = Self::new(max_concurrent, time_cost);
        (engine.clone(), engine.operations)
    }

    /// 在有界阻塞线程池上将明文密码哈希为 Argon2id PHC。
    ///
    /// 操作计数器会在等待许可前递增。
    ///
    /// # Errors
    ///
    /// 信号量关闭、阻塞任务失败、无法编码盐值/Argon2 参数或 Argon2 哈希失败时，返回 [`AppError`]。
    pub async fn hash(&self, password: String) -> Result<String, AppError> {
        self.operations.hashes.fetch_add(1, Ordering::Relaxed);
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
        let time_cost = self.time_cost;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            hash_blocking(&password, time_cost)
        })
        .await
        .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?
    }

    /// 在有界阻塞线程池上根据 PHC 字符串验证明文密码。
    ///
    /// 可观测验证计数器会在等待工作器许可前递增，因此包括后来在等待、启动或解析时失败的调用。
    /// 格式正确但不匹配的 PHC 返回 `Ok(false)`。
    ///
    /// # Errors
    ///
    /// 信号量关闭、阻塞任务失败或无法解析 `phc` 时返回 [`AppError`]。密码不匹配本身不是错误。
    pub async fn verify(&self, password: String, phc: String) -> Result<bool, AppError> {
        self.operations
            .verifications
            .fetch_add(1, Ordering::Relaxed);
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let parsed = PasswordHash::new(&phc)
                .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
            Ok(Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok())
        })
        .await
        .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?
    }

    #[must_use]
    /// 返回用于新哈希的钳制后 Argon2id 迭代次数。
    pub const fn time_cost(&self) -> u32 {
        self.time_cost
    }

    #[must_use]
    /// 克隆用于均衡未知账户登录验证的预计算 PHC。
    pub fn dummy_phc(&self) -> String {
        self.dummy_phc.to_string()
    }
}

fn hash_blocking(password: &str, time_cost: u32) -> Result<String, AppError> {
    let salt_bytes =
        random::sha256(format!("{}:{}", uuid::Uuid::now_v7(), password.len()).as_bytes());
    let salt = SaltString::encode_b64(&salt_bytes[..16])
        .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
    let params = Params::new(MEMORY_KIB, time_cost.max(MIN_TIME_COST), PARALLELISM, None)
        .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))
}

#[derive(Debug, Serialize)]
/// 用于运维配置的本地 Argon2id 验证校准结果。
pub struct CalibrationReport {
    /// 密码算法，始终为 `"argon2id"`。
    pub algorithm: &'static str,
    /// 被测操作，始终为 `"verify"`。
    pub operation: &'static str,
    /// 样本使用的固定内存成本。
    pub memory_kib: u32,
    /// 样本使用的固定通道数。
    pub parallelism: u32,
    /// Core 认为安全的最低迭代次数。
    pub minimum_time_cost: u32,
    /// 要在此主机上配置的选定迭代次数。
    pub suggested_time_cost: u32,
    /// 选定样本的验证耗时，单位为毫秒。
    pub measured_ms: u128,
    /// 包含端点的目标时长下限，250 ms。
    pub target_min_ms: u128,
    /// 包含端点的目标时长上限，500 ms。
    pub target_max_ms: u128,
}

/// 测量从最小成本到成本 32 的 Argon2id 验证，并推荐一个主机值。
///
/// 一旦某次验证达到 250 ms，采样便停止；报告优先选择 250–500 ms 目标窗口中的第一个样本，
/// 再选择 [`recommend_time_cost`] 确定的最接近可用回退值。此函数会同步执行消耗 CPU 和内存的工作。
///
/// # Errors
///
/// 样本哈希/PHC 解析/验证失败或未产生样本时，返回 [`AppError`]。
pub fn calibrate() -> Result<CalibrationReport, AppError> {
    let mut samples = Vec::new();
    for time_cost in MIN_TIME_COST..=32 {
        let phc = hash_blocking("mediaflow calibration sample", time_cost)?;
        let parsed = PasswordHash::new(&phc)
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
        let started = Instant::now();
        Argon2::default()
            .verify_password(b"mediaflow calibration sample", &parsed)
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
        let elapsed = started.elapsed();
        samples.push((time_cost, elapsed));
        if elapsed >= Duration::from_millis(250) {
            break;
        }
    }
    let (suggested_time_cost, measured) = recommend_time_cost(&samples)
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "calibration produced no samples"))?;
    Ok(CalibrationReport {
        algorithm: "argon2id",
        operation: "verify",
        memory_kib: MEMORY_KIB,
        parallelism: PARALLELISM,
        minimum_time_cost: MIN_TIME_COST,
        suggested_time_cost,
        measured_ms: measured.as_millis(),
        target_min_ms: 250,
        target_max_ms: 500,
    })
}

#[must_use]
/// 选择第一个 250–500 ms 样本；否则选择低于目标的最慢样本或可用的最快样本。
///
/// 返回的成本会钳制到 [`MIN_TIME_COST`]；空样本集返回 `None`。
pub fn recommend_time_cost(samples: &[(u32, Duration)]) -> Option<(u32, Duration)> {
    samples
        .iter()
        .copied()
        .find(|(_, duration)| {
            *duration >= Duration::from_millis(250) && *duration <= Duration::from_millis(500)
        })
        .or_else(|| {
            samples
                .iter()
                .copied()
                .filter(|(_, duration)| *duration < Duration::from_millis(250))
                .max_by_key(|(_, duration)| *duration)
        })
        .or_else(|| {
            samples
                .iter()
                .copied()
                .min_by_key(|(_, duration)| *duration)
        })
        .map(|(cost, duration)| (cost.max(MIN_TIME_COST), duration))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_calibration_selects_first_sample_in_target() {
        let samples = [
            (2, Duration::from_millis(180)),
            (3, Duration::from_millis(310)),
            (4, Duration::from_millis(440)),
        ];
        assert_eq!(
            recommend_time_cost(&samples),
            Some((3, Duration::from_millis(310)))
        );
    }

    #[test]
    fn configured_time_cost_accepts_calibration_output_without_dropping_below_minimum() {
        assert_eq!(configured_time_cost(Some("11")), 11);
        assert_eq!(configured_time_cost(Some("1")), MIN_TIME_COST);
        assert_eq!(configured_time_cost(Some("invalid")), DEFAULT_TIME_COST);
        assert_eq!(configured_time_cost(None), DEFAULT_TIME_COST);
    }

    #[tokio::test]
    async fn hashing_waits_for_the_bounded_worker_permit() {
        let engine = PasswordEngine::new(1, MIN_TIME_COST);
        let held = engine.semaphore.clone().acquire_owned().await.unwrap();
        let pending = tokio::spawn({
            let engine = engine.clone();
            async move { engine.hash("bounded password operation".to_owned()).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !pending.is_finished(),
            "hashing must wait instead of bypassing the bounded worker pool"
        );
        drop(held);
        assert!(pending.await.unwrap().unwrap().starts_with("$argon2id$"));
    }
}
