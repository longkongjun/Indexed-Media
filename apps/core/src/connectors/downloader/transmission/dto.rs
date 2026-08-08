pub(super) struct SessionInfo {
    pub product_version: String,
    pub api_version: String,
}

pub(super) struct TorrentFields {
    pub hash: String,
    pub status: i64,
    pub progress: f64,
    pub error: i64,
}
