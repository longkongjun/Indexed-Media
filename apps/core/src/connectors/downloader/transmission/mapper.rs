use crate::connectors::downloader::port::{
    DownloadSourceError, RemoteDownloadSnapshot, RemoteDownloadStatus,
};

use super::dto::TorrentFields;

pub(super) fn map_torrent(
    torrent: TorrentFields,
) -> Result<RemoteDownloadSnapshot, DownloadSourceError> {
    let TorrentFields {
        hash,
        status,
        progress,
        error,
    } = torrent;
    if !valid_hash(&hash) || !progress.is_finite() || !(0.0..=1.0).contains(&progress) || error < 0
    {
        return Err(DownloadSourceError::InvalidResponse);
    }
    let status = if error > 0 {
        RemoteDownloadStatus::Failed
    } else {
        match status {
            0 if progress >= 1.0 => RemoteDownloadStatus::Completed,
            0 => RemoteDownloadStatus::Paused,
            1..=3 => RemoteDownloadStatus::Queued,
            4 => RemoteDownloadStatus::Downloading,
            5 | 6 => RemoteDownloadStatus::Completed,
            _ => RemoteDownloadStatus::Unknown,
        }
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
