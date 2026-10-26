use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 测试进展看门狗；只有单调计数增加才能延后期限。
pub struct ProgressDeadline {
    last: u64,
    deadline: Instant,
    idle: Duration,
}

impl ProgressDeadline {
    pub fn new(initial: u64, idle: Duration, now: Instant) -> Self {
        Self {
            last: initial,
            deadline: now + idle,
            idle,
        }
    }

    pub fn observe(&mut self, value: u64, now: Instant) -> bool {
        if value > self.last {
            self.last = value;
            self.deadline = now + self.idle;
        }
        now < self.deadline
    }
}

pub async fn with_progress_timeout<F: Future>(
    future: F,
    progress: &AtomicU64,
    idle: Duration,
) -> Result<F::Output, &'static str> {
    let mut deadline =
        ProgressDeadline::new(progress.load(Ordering::Acquire), idle, Instant::now());
    let mut poll = tokio::time::interval(idle.min(Duration::from_millis(10)));
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline.deadline.into()) => {
                if !deadline.observe(progress.load(Ordering::Acquire), Instant::now()) {
                    return Err("no forward progress before deadline");
                }
            }
            _ = poll.tick() => {
                if !deadline.observe(progress.load(Ordering::Acquire), Instant::now()) {
                    return Err("no forward progress before deadline");
                }
            }
            result = &mut future => return Ok(result),
        }
    }
}
