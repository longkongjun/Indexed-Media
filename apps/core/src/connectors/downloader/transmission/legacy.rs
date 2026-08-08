use serde::Deserialize;
use serde_json::json;

use crate::connectors::downloader::port::DownloadSourceError;

use super::dto::{SessionInfo, TorrentFields};

pub(super) fn session_request() -> Result<Vec<u8>, DownloadSourceError> {
    encode(&json!({
        "method":"session-get",
        "arguments":{"fields":["version","rpc-version-semver"]}
    }))
}

pub(super) fn add_request(source: &str) -> Result<Vec<u8>, DownloadSourceError> {
    encode(&json!({"method":"torrent-add","arguments":{"filename":source}}))
}

pub(super) fn fetch_request(remote_ids: &[String]) -> Result<Vec<u8>, DownloadSourceError> {
    encode(&json!({
        "method":"torrent-get",
        "arguments":{
            "ids":remote_ids,
            "fields":["hashString","status","percentDone","error"]
        }
    }))
}

pub(super) fn parse_session(body: &[u8]) -> Result<SessionInfo, DownloadSourceError> {
    let response: LegacyEnvelope<LegacySession> = decode(body)?;
    response.success()?;
    validate_text(&response.arguments.version)?;
    validate_text(&response.arguments.rpc_version_semver)?;
    Ok(SessionInfo {
        product_version: response.arguments.version,
        api_version: response.arguments.rpc_version_semver,
    })
}

pub(super) fn parse_add(body: &[u8]) -> Result<String, DownloadSourceError> {
    let response: LegacyEnvelope<LegacyAddArguments> = decode(body)?;
    response.success()?;
    match (
        response.arguments.torrent_added,
        response.arguments.torrent_duplicate,
    ) {
        (Some(value), None) | (None, Some(value)) => Ok(value.hash),
        _ => Err(DownloadSourceError::InvalidResponse),
    }
}

pub(super) fn parse_fetch(body: &[u8]) -> Result<Vec<TorrentFields>, DownloadSourceError> {
    let response: LegacyEnvelope<LegacyTorrents> = decode(body)?;
    response.success()?;
    Ok(response
        .arguments
        .torrents
        .into_iter()
        .map(|torrent| TorrentFields {
            hash: torrent.hash,
            status: torrent.status,
            progress: torrent.progress,
            error: torrent.error,
        })
        .collect())
}

#[derive(Deserialize)]
struct LegacyEnvelope<T> {
    arguments: T,
    result: String,
}

impl<T> LegacyEnvelope<T> {
    fn success(&self) -> Result<(), DownloadSourceError> {
        if self.result == "success" {
            Ok(())
        } else {
            Err(DownloadSourceError::InvalidResponse)
        }
    }
}

#[derive(Deserialize)]
struct LegacySession {
    #[serde(rename = "rpc-version-semver")]
    rpc_version_semver: String,
    version: String,
}

#[derive(Deserialize)]
struct LegacyAddArguments {
    #[serde(rename = "torrent-added")]
    torrent_added: Option<LegacyAdded>,
    #[serde(rename = "torrent-duplicate")]
    torrent_duplicate: Option<LegacyAdded>,
}

#[derive(Deserialize)]
struct LegacyAdded {
    #[serde(rename = "hashString")]
    hash: String,
}

#[derive(Deserialize)]
struct LegacyTorrents {
    torrents: Vec<LegacyTorrent>,
}

#[derive(Deserialize)]
struct LegacyTorrent {
    #[serde(rename = "hashString")]
    hash: String,
    status: i64,
    #[serde(rename = "percentDone")]
    progress: f64,
    error: i64,
}

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, DownloadSourceError> {
    serde_json::to_vec(value).map_err(|_| DownloadSourceError::InvalidResponse)
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, DownloadSourceError> {
    serde_json::from_slice(body).map_err(|_| DownloadSourceError::InvalidResponse)
}

fn validate_text(value: &str) -> Result<(), DownloadSourceError> {
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
        Err(DownloadSourceError::InvalidResponse)
    } else {
        Ok(())
    }
}
