use crate::connectors::downloader::port::{
    DownloadSourceError, RemoteDownloadSnapshot, RemoteDownloadStatus,
};

use super::dto::TorrentDto;

pub(super) fn map_torrent(
    torrent: TorrentDto,
) -> Result<RemoteDownloadSnapshot, DownloadSourceError> {
    let TorrentDto {
        hash,
        state,
        progress,
    } = torrent;
    if !valid_hash(&hash) || !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
        return Err(DownloadSourceError::InvalidResponse);
    }
    let status = match state.as_str() {
        "queuedDL" | "metaDL" | "checkingDL" | "checkingResumeData" | "allocating" | "moving" => {
            RemoteDownloadStatus::Queued
        }
        "downloading" | "forcedDL" | "stalledDL" => RemoteDownloadStatus::Downloading,
        "pausedDL" | "stoppedDL" => RemoteDownloadStatus::Paused,
        "uploading" | "forcedUP" | "stalledUP" | "queuedUP" | "checkingUP" | "pausedUP"
        | "stoppedUP" => RemoteDownloadStatus::Completed,
        "error" | "missingFiles" => RemoteDownloadStatus::Failed,
        _ => RemoteDownloadStatus::Unknown,
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let progress_basis_points = (progress * 10_000.0).round() as u16;
    Ok(RemoteDownloadSnapshot {
        remote_id: hash.to_ascii_lowercase(),
        status,
        progress_basis_points,
    })
}

pub(super) fn valid_hash(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
