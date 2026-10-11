//! 能力登记表：每个能力的副作用等级、允许的执行模式与输入字段白名单。
//!
//! 输入校验分两层：字段白名单（本表声明）拒绝未登记字段；值检查拒绝绝对路径、shell
//! 控制序列、SQL 语句特征与凭据字段——AI 输入不可信，Core 不信任调用方已完成校验。

use serde_json::{Map, Value};

use crate::capability::model::{CapabilityExecutionMode, CapabilityName};
use crate::shared::error::{AppError, ErrorCode};

/// 一个已登记能力的执行约束。
#[derive(Clone, Copy, Debug)]
pub struct CapabilitySpec {
    /// 能力名。
    pub name: CapabilityName,
    /// 允许的最强执行模式；A1 阶段全部为只读或计划预览。
    pub allowed_modes: &'static [CapabilityExecutionMode],
    /// 允许出现的输入字段；未列字段一律拒绝。
    pub input_fields: &'static [&'static str],
}

/// A1 阶段登记的能力清单；副作用能力自 A2 起逐个登记。
pub const REGISTRY: &[CapabilitySpec] = &[
    CapabilitySpec {
        name: CapabilityName::MediaTaskStatus,
        allowed_modes: &[CapabilityExecutionMode::ReadOnly],
        input_fields: &["task_id"],
    },
    CapabilitySpec {
        name: CapabilityName::MediaMatchCandidates,
        allowed_modes: &[CapabilityExecutionMode::ReadOnly],
        input_fields: &["task_id", "case_id"],
    },
    CapabilitySpec {
        name: CapabilityName::MediaCreatePlan,
        allowed_modes: &[CapabilityExecutionMode::PlanOnly],
        input_fields: &["task_id", "dry_run"],
    },
];

/// 查找能力描述；未登记能力是稳定拒绝。
#[must_use]
pub fn lookup(name: CapabilityName) -> Option<&'static CapabilitySpec> {
    REGISTRY.iter().find(|spec| spec.name == name)
}

/// 校验调用输入：字段白名单 + 危险值模式。
///
/// # Errors
///
/// 输入含未登记字段、绝对路径、shell 控制序列、SQL 语句特征或凭据字段时返回
/// 稳定校验错误。
pub fn validate_input(spec: &CapabilitySpec, input: &Map<String, Value>) -> Result<(), AppError> {
    for key in input.keys() {
        if !spec.input_fields.contains(&key.as_str()) {
            return Err(AppError::new(
                ErrorCode::ValidationFailed,
                "capability input field is not registered",
            ));
        }
    }
    for (key, value) in input {
        reject_credential_keys(key)?;
        reject_unsafe_values(key, value)?;
    }
    Ok(())
}

/// 凭据键名清单；AI 输入中出现即拒绝。
const FORBIDDEN_CREDENTIAL_KEYS: &[&str] = &[
    "password",
    "token",
    "secret",
    "credential",
    "api_key",
    "apikey",
];

/// SQL 语句骨架特征（小写）；AI 输入不得携带语句形态的值。
const SQL_STATEMENT_PATTERNS: &[&str] = &[
    "select ",
    "insert into",
    "update ",
    "delete from",
    "drop table",
    "alter table",
    "create table",
    "--",
    "/*",
];

/// 凭据不得进入 AI 输入；键名即拒绝。
fn reject_credential_keys(key: &str) -> Result<(), AppError> {
    let lowered = key.to_ascii_lowercase();
    if FORBIDDEN_CREDENTIAL_KEYS
        .iter()
        .any(|needle| lowered.contains(needle))
    {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "capability input must not carry credentials",
        ));
    }
    Ok(())
}

/// 递归拒绝危险字符串值：绝对路径、路径逃逸、shell 控制序列与 SQL 语句特征。
fn reject_unsafe_values(path: &str, value: &Value) -> Result<(), AppError> {
    match value {
        Value::String(text) => reject_unsafe_text(path, text),
        Value::Array(items) => {
            for item in items {
                reject_unsafe_values(path, item)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (key, child) in map {
                reject_credential_keys(key)?;
                reject_unsafe_values(key, child)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// 字符串值的危险模式检查；命中即稳定拒绝，消息不回显值本身。
fn reject_unsafe_text(path: &str, text: &str) -> Result<(), AppError> {
    let _ = path;
    // 绝对路径与路径逃逸：AI 不得提交宿主路径。
    if text.starts_with('/')
        || text.starts_with('\\')
        || text.starts_with("~/")
        || text.contains("..")
    {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "capability input must not contain host paths",
        ));
    }
    // shell 控制序列。
    if text.contains('`')
        || text.contains("$(")
        || text.contains("; ")
        || text.contains("&&")
        || text.contains("||")
        || text.contains('|')
    {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "capability input must not contain shell control sequences",
        ));
    }
    // SQL 语句特征（大小写不敏感的语句骨架 + 行内注释）。
    let lowered = text.to_ascii_lowercase();
    if SQL_STATEMENT_PATTERNS
        .iter()
        .any(|needle| lowered.contains(needle))
    {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "capability input must not contain SQL statements",
        ));
    }
    Ok(())
}
