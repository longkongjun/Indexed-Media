#[path = "common/progress.rs"]
mod progress;

use progress::{ProgressDeadline, with_progress_timeout};
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

#[test]
fn forward_progress_extends_idle_limit_without_a_total_duration_limit() {
    let start = Instant::now();
    let mut deadline = ProgressDeadline::new(0, Duration::from_secs(5), start);
    for (seconds, count) in [(4, 1), (8, 2), (12, 3)] {
        assert!(deadline.observe(count, start + Duration::from_secs(seconds)));
    }
    assert!(!deadline.observe(3, start + Duration::from_secs(17)));
}

#[test]
fn repeated_observations_and_regressions_do_not_hide_a_stall() {
    let start = Instant::now();
    let mut deadline = ProgressDeadline::new(100, Duration::from_secs(5), start);
    assert!(deadline.observe(100, start + Duration::from_secs(2)));
    assert!(deadline.observe(99, start + Duration::from_secs(4)));
    assert!(!deadline.observe(100, start + Duration::from_secs(5)));
}

#[tokio::test]
async fn a_real_pending_future_fails_without_progress() {
    assert!(
        with_progress_timeout(
            std::future::pending::<()>(),
            &AtomicU64::new(0),
            Duration::from_millis(20)
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn a_completed_future_keeps_its_result() {
    assert_eq!(
        with_progress_timeout(
            std::future::ready(42),
            &AtomicU64::new(0),
            Duration::from_secs(5)
        )
        .await,
        Ok(42)
    );
}
