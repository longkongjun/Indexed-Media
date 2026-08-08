use mediaflow_core::discovery::watcher::{
    COALESCE_WINDOW_US, WatchCoalescer, WatchHint, WatchIngress, WatchMapError, map_event_path,
};

#[test]
fn duplicate_and_out_of_order_hints_coalesce_by_inbox_and_raw_path_for_two_seconds() {
    let inbox = uuid::Uuid::now_v7();
    let other = uuid::Uuid::now_v7();
    let mut coalescer = WatchCoalescer::default();
    assert!(coalescer.push(WatchHint::new(inbox, b"movie.mkv".to_vec(), 1_000_000)));
    assert!(coalescer.push(WatchHint::new(inbox, b"movie.mkv".to_vec(), 500_000)));
    assert!(coalescer.push(WatchHint::new(inbox, b"movie.mkv".to_vec(), 2_000_000)));
    assert!(coalescer.push(WatchHint::new(other, b"movie.mkv".to_vec(), 1_500_000)));

    assert!(
        coalescer
            .drain_ready(1_000_000 + COALESCE_WINDOW_US - 1)
            .is_empty()
    );
    let ready = coalescer.drain_ready(1_000_000 + COALESCE_WINDOW_US);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].inbox_directory_id, inbox);
    assert_eq!(ready[0].relative_path_bytes, b"movie.mkv");
    assert_eq!(ready[0].observed_at_us, 2_000_000);
    assert_eq!(coalescer.len(), 1);
}

#[test]
fn unique_path_storm_hits_a_second_hard_bound_while_duplicates_remain_mergeable() {
    let inbox = uuid::Uuid::now_v7();
    let mut coalescer = WatchCoalescer::with_capacity(2);
    assert!(coalescer.push(WatchHint::new(inbox, b"one.mkv".to_vec(), 1)));
    assert!(coalescer.push(WatchHint::new(inbox, b"two.mkv".to_vec(), 2)));
    assert!(coalescer.push(WatchHint::new(inbox, b"one.mkv".to_vec(), 3)));
    assert!(!coalescer.push(WatchHint::new(inbox, b"three.mkv".to_vec(), 4)));
    assert_eq!(coalescer.len(), 2);
    coalescer.clear();
    assert!(coalescer.is_empty());
}

#[tokio::test]
async fn callback_ingress_is_bounded_and_overflow_is_a_sticky_reconcile_signal() {
    let inbox = uuid::Uuid::now_v7();
    let (ingress, mut receiver) = WatchIngress::bounded(1);
    assert!(ingress.try_send(WatchHint::new(inbox, b"one.mkv".to_vec(), 1)));
    assert!(!ingress.try_send(WatchHint::new(inbox, b"two.mkv".to_vec(), 2)));
    assert!(ingress.take_overflow());
    assert!(!ingress.take_overflow());
    assert_eq!(
        receiver.recv().await.unwrap().relative_path_bytes,
        b"one.mkv"
    );
}

#[test]
fn event_path_mapping_preserves_non_utf8_and_rejects_outside_or_root_paths() {
    let root = std::path::Path::new("/srv/inbox");
    assert_eq!(
        map_event_path(root, std::path::Path::new("/srv/inbox/movies/a.mkv")).unwrap(),
        b"movies/a.mkv"
    );
    assert_eq!(
        map_event_path(root, std::path::Path::new("/srv/other/a.mkv")),
        Err(WatchMapError::OutsideInbox)
    );
    assert_eq!(map_event_path(root, root), Err(WatchMapError::InboxRoot));

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let event = std::path::PathBuf::from(std::ffi::OsString::from_vec(
            b"/srv/inbox/damaged-\xff.mkv".to_vec(),
        ));
        assert_eq!(map_event_path(root, &event).unwrap(), b"damaged-\xff.mkv");
    }
}
