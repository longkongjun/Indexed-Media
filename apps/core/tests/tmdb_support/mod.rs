#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;

pub struct FakeTmdb {
    pub origin: url::Url,
    task: tokio::task::JoinHandle<()>,
}

impl FakeTmdb {
    pub async fn spawn(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fake TMDB listener");
        let address = listener.local_addr().expect("fake TMDB address");
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("fake TMDB server");
        });
        Self {
            origin: url::Url::parse(&format!("http://{address}/3/")).unwrap(),
            task,
        }
    }
}

impl Drop for FakeTmdb {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct RequestProbe {
    pub calls: AtomicUsize,
    pub active: AtomicUsize,
    pub maximum_active: AtomicUsize,
    pub authorization: std::sync::Mutex<Vec<String>>,
    pub paths: std::sync::Mutex<Vec<String>>,
}

impl RequestProbe {
    pub fn enter(self: &Arc<Self>) -> ActiveRequest {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum_active.fetch_max(active, Ordering::SeqCst);
        ActiveRequest(self.clone())
    }
}

pub struct ActiveRequest(Arc<RequestProbe>);

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
