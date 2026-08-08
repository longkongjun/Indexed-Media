use serde::Deserialize;
use serde_json::json;

use crate::connectors::downloader::port::DownloadSourceError;

use super::dto::{SessionInfo, TorrentFields};

pub(super) fn session_request() -> Result<Vec<u8>, DownloadSourceError> {
    encode(&json!({
        "jsonrpc":"2.0","id":1,"method":"session_get",
        "params":{"fields":["version","rpc_version_semver"]}
    }))
}

pub(super) fn add_request(source: &str) -> Result<Vec<u8>, DownloadSourceError> {
    encode(&json!({
        "jsonrpc":"2.0","id":2,"method":"torrent_add","params":{"filename":source}
    }))
}

pub(super) fn fetch_request(remote_ids: &[String]) -> Result<Vec<u8>, DownloadSourceError> {
    encode(&json!({
        "jsonrpc":"2.0","id":2,"method":"torrent_get",
        "params":{
            "ids":remote_ids,
            "fields":["hash_string","status","percent_done","error"]
        }
    }))
}

pub(super) fn parse_session(body: &[u8]) -> Result<SessionInfo, DownloadSourceError> {
    let response: JsonEnvelope<JsonSession> = decode(body)?;
    let result = response.result()?;
    validate_text(&result.version)?;
    validate_text(&result.rpc_version_semver)?;
    Ok(SessionInfo {
        product_version: result.version,
        api_version: result.rpc_version_semver,
    })
}

pub(super) fn parse_add(body: &[u8]) -> Result<String, DownloadSourceError> {
    let response: JsonEnvelope<JsonAddResult> = decode(body)?;
    let result = response.result()?;
    match (result.torrent_added, result.torrent_duplicate) {
        (Some(value), None) | (None, Some(value)) => Ok(value.hash_string),
        _ => Err(DownloadSourceError::InvalidResponse),
    }
}

pub(super) fn parse_fetch(body: &[u8]) -> Result<Vec<TorrentFields>, DownloadSourceError> {
    let response: JsonEnvelope<JsonTorrents> = decode(body)?;
    Ok(response
        .result()?
        .torrents
        .into_iter()
        .map(JsonTorrent::into_fields)
        .collect())
}

#[derive(Deserialize)]
struct JsonEnvelope<T> {
    jsonrpc: String,
    id: serde_json::Value,
    result: Option<T>,
    error: Option<serde_json::Value>,
}

impl<T> JsonEnvelope<T> {
    fn result(self) -> Result<T, DownloadSourceError> {
        if self.jsonrpc != "2.0" || self.id.is_null() || self.error.is_some() {
            return Err(DownloadSourceError::InvalidResponse);
        }
        self.result.ok_or(DownloadSourceError::InvalidResponse)
    }
}

#[derive(Deserialize)]
struct JsonSession {
    rpc_version_semver: String,
    version: String,
}

#[derive(Deserialize)]
struct JsonAddResult {
    torrent_added: Option<JsonAdded>,
    torrent_duplicate: Option<JsonAdded>,
}

#[derive(Deserialize)]
struct JsonAdded {
    hash_string: String,
}

#[derive(Deserialize)]
struct JsonTorrents {
    torrents: Vec<JsonTorrent>,
}

#[derive(Deserialize)]
struct JsonTorrent {
    hash_string: String,
    status: i64,
    percent_done: f64,
    error: i64,
}

impl JsonTorrent {
    fn into_fields(self) -> TorrentFields {
        TorrentFields {
            hash: self.hash_string,
            status: self.status,
            progress: self.percent_done,
            error: self.error,
        }
    }
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
