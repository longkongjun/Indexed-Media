#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mediaflow_core::connectors::downloader::model::{
    CreateDownloadTaskCommand, DownloaderConnectionInput, DownloaderKind,
};
use mediaflow_core::connectors::downloader::port::{
    AddDownloadRequest, DownloadSource, DownloadSourceError, DownloaderEndpoint,
    RemoteDownloadQuery, RemoteDownloadRef, RemoteDownloadSnapshot,
};
use mediaflow_core::connectors::model::SecretString;

pub const SOURCE: &str = "magnet:?xt=urn:btih:RUNTIME_SOURCE_MUST_NOT_LEAK";

#[derive(Default)]
struct FakeState {
    correlations: HashMap<String, Vec<RemoteDownloadSnapshot>>,
    snapshots: HashMap<String, RemoteDownloadSnapshot>,
    add_results: VecDeque<Result<RemoteDownloadRef, DownloadSourceError>>,
    fetch_errors: VecDeque<DownloadSourceError>,
    add_calls: Vec<String>,
    queries: Vec<RemoteDownloadQuery>,
}

#[derive(Clone, Default)]
pub struct FakeDownloadSource {
    state: Arc<Mutex<FakeState>>,
}

impl FakeDownloadSource {
    pub fn correlation(&self, tag: String, snapshots: Vec<RemoteDownloadSnapshot>) {
        self.state
            .lock()
            .unwrap()
            .correlations
            .insert(tag, snapshots);
    }

    pub fn snapshot(&self, snapshot: RemoteDownloadSnapshot) {
        self.state
            .lock()
            .unwrap()
            .snapshots
            .insert(snapshot.remote_id.clone(), snapshot);
    }

    pub fn fetch_error(&self, error: DownloadSourceError) {
        self.state.lock().unwrap().fetch_errors.push_back(error);
    }

    pub fn add_result(&self, result: Result<RemoteDownloadRef, DownloadSourceError>) {
        self.state.lock().unwrap().add_results.push_back(result);
    }

    pub fn add_count(&self) -> usize {
        self.state.lock().unwrap().add_calls.len()
    }

    pub fn queries(&self) -> Vec<RemoteDownloadQuery> {
        self.state.lock().unwrap().queries.clone()
    }
}

#[async_trait]
impl DownloadSource for FakeDownloadSource {
    async fn probe(
        &self,
        _endpoint: DownloaderEndpoint<'_>,
    ) -> Result<
        mediaflow_core::connectors::downloader::model::DownloaderCapabilities,
        DownloadSourceError,
    > {
        Ok(
            mediaflow_core::connectors::downloader::model::DownloaderCapabilities {
                manual_add: true,
                task_monitoring: true,
                product_version: "test".to_owned(),
                api_version: "test".to_owned(),
            },
        )
    }

    async fn add(
        &self,
        _endpoint: DownloaderEndpoint<'_>,
        request: AddDownloadRequest<'_>,
    ) -> Result<RemoteDownloadRef, DownloadSourceError> {
        let mut state = self.state.lock().unwrap();
        state.add_calls.push(request.correlation_tag.to_owned());
        state
            .add_results
            .pop_front()
            .unwrap_or(Err(DownloadSourceError::InvalidResponse))
    }

    async fn fetch(
        &self,
        _endpoint: DownloaderEndpoint<'_>,
        query: RemoteDownloadQuery,
    ) -> Result<Vec<RemoteDownloadSnapshot>, DownloadSourceError> {
        let mut state = self.state.lock().unwrap();
        state.queries.push(query.clone());
        if let Some(error) = state.fetch_errors.pop_front() {
            return Err(error);
        }
        if let Some(tag) = query.correlation_tag {
            return Ok(state.correlations.get(&tag).cloned().unwrap_or_default());
        }
        Ok(query
            .remote_ids
            .iter()
            .filter_map(|id| state.snapshots.get(id).cloned())
            .collect())
    }
}

pub fn connection_input() -> DownloaderConnectionInput {
    DownloaderConnectionInput {
        kind: DownloaderKind::Qbittorrent,
        display_name: "Runtime qBit".to_owned(),
        base_url: "https://download.test/qbit".to_owned(),
        username: SecretString::new("user".to_owned()),
        password: SecretString::new("password".to_owned()),
        enabled: true,
    }
}

pub fn task_command(connection_id: uuid::Uuid, display_name: &str) -> CreateDownloadTaskCommand {
    CreateDownloadTaskCommand {
        connection_id,
        source: SecretString::new(SOURCE.to_owned()),
        display_name: display_name.to_owned(),
    }
}
