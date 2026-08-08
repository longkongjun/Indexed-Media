use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct AddTorrentsResponse {
    pub added_torrent_ids: Vec<String>,
    pub failure_count: u32,
    pub pending_count: u32,
    pub success_count: u32,
}

#[derive(Deserialize)]
pub(super) struct TorrentDto {
    pub hash: String,
    pub state: String,
    pub progress: f64,
}
