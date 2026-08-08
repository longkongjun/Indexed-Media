use chrono::{DateTime, Utc};

#[must_use]
/// 返回当前 UTC 挂钟时刻。
///
/// 需要确定性时间或单调时间的调用方必须自行提供时钟。
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}
