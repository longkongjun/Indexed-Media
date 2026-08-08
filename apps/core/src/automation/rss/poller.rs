use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::platform::secrets::SecretBytes;

use super::client::{FeedClient, FeedClientError, FeedResponse, RssCursor};
use super::parser::{FeedFormat, FeedParser};

/// One claimed RSS source needed for a bounded poll.
pub struct FeedPollSource {
    /// Stable source identifier.
    pub id: Uuid,
    /// Expected source configuration version.
    pub config_version: i64,
    /// Target downloader connection.
    pub downloader_connection_id: Uuid,
    /// Short-lived decrypted feed URL.
    pub feed_url: SecretBytes,
    /// Previously committed conditional cursor.
    pub cursor: Option<RssCursor>,
}

impl std::fmt::Debug for FeedPollSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeedPollSource")
            .field("id", &self.id)
            .field("config_version", &self.config_version)
            .field("downloader_connection_id", &self.downloader_connection_id)
            .field("feed_url", &"[REDACTED]")
            .field("cursor", &self.cursor)
            .finish()
    }
}

/// Secret-bearing event draft accepted atomically with the feed cursor.
pub struct AutomationEventDraft {
    /// Per-source GUID/ID/source dedup digest.
    pub dedup_key: [u8; 32],
    /// Target downloader connection for `create-download`.
    pub downloader_connection_id: Uuid,
    /// Bounded safe task display name.
    pub display_name: String,
    source: SecretBytes,
}

impl AutomationEventDraft {
    #[must_use]
    /// Borrow the source only at the encrypted event store boundary.
    pub const fn source(&self) -> &SecretBytes {
        &self.source
    }

    #[must_use]
    /// Consume the draft and transfer its secret source.
    pub fn into_source(self) -> SecretBytes {
        self.source
    }
}

impl std::fmt::Debug for AutomationEventDraft {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AutomationEventDraft")
            .field("dedup_key", &hex::encode(self.dedup_key))
            .field("downloader_connection_id", &self.downloader_connection_id)
            .field("display_name", &self.display_name)
            .field("source", &"[REDACTED]")
            .finish()
    }
}

/// Cursor, health observation, and event drafts that must commit atomically.
pub struct FeedPollCommit {
    /// New or retained conditional cursor.
    pub cursor: Option<RssCursor>,
    /// Detected format for modified responses.
    pub format: Option<FeedFormat>,
    /// Invalid entries ignored safely.
    pub ignored_item_count: u16,
    /// Valid create-download drafts.
    pub drafts: Vec<AutomationEventDraft>,
}

impl std::fmt::Debug for FeedPollCommit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeedPollCommit")
            .field("cursor", &self.cursor)
            .field("format", &self.format)
            .field("ignored_item_count", &self.ignored_item_count)
            .field("draft_count", &self.drafts.len())
            .finish()
    }
}

#[async_trait]
/// Atomic persistence boundary implemented by the durable event store.
pub trait FeedPollCommitPort: Send + Sync {
    /// Commit deduplicated drafts and the conditional cursor in one transaction.
    async fn commit(
        &self,
        source_id: Uuid,
        expected_config_version: i64,
        commit: FeedPollCommit,
    ) -> Result<usize, FeedClientError>;

    /// Commit a redacted source health failure without advancing its cursor.
    async fn commit_failure(
        &self,
        source_id: Uuid,
        expected_config_version: i64,
        failure: FeedClientError,
    ) -> Result<(), FeedClientError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Safe result of one committed source poll.
pub struct FeedPollResult {
    /// Newly accepted event count after deduplication.
    pub accepted_event_count: usize,
    /// Invalid source entries ignored by the parser.
    pub ignored_item_count: u16,
    /// Whether the upstream response was not modified.
    pub not_modified: bool,
}

/// Coordinates conditional fetch, bounded parse, and atomic persistence.
#[derive(Clone)]
pub struct FeedPoller {
    client: Arc<dyn FeedClient>,
    commits: Arc<dyn FeedPollCommitPort>,
    parser: FeedParser,
}

impl std::fmt::Debug for FeedPoller {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("FeedPoller").finish_non_exhaustive()
    }
}

impl FeedPoller {
    #[must_use]
    /// Build a poller around explicit transport and atomic commit ports.
    pub fn new(client: Arc<dyn FeedClient>, commits: Arc<dyn FeedPollCommitPort>) -> Self {
        Self {
            client,
            commits,
            parser: FeedParser::default(),
        }
    }

    /// Poll one already claimed source.
    ///
    /// # Errors
    ///
    /// Returns stable transport, parsing, or atomic commit failures. A failed commit never owns
    /// the new cursor.
    pub async fn poll(&self, source: FeedPollSource) -> Result<FeedPollResult, FeedClientError> {
        let response = match self
            .client
            .fetch(&source.feed_url, source.cursor.as_ref())
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.commits
                    .commit_failure(source.id, source.config_version, error)
                    .await?;
                return Err(error);
            }
        };
        match response {
            FeedResponse::NotModified { cursor } => {
                let accepted = self
                    .commits
                    .commit(
                        source.id,
                        source.config_version,
                        FeedPollCommit {
                            cursor,
                            format: None,
                            ignored_item_count: 0,
                            drafts: Vec::new(),
                        },
                    )
                    .await?;
                Ok(FeedPollResult {
                    accepted_event_count: accepted,
                    ignored_item_count: 0,
                    not_modified: true,
                })
            }
            FeedResponse::Modified { body, cursor, .. } => {
                let parsed = match self.parser.parse(&body) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        let failure = match error {
                            super::parser::FeedParseError::TooLarge => {
                                FeedClientError::ResponseTooLarge
                            }
                            _ => FeedClientError::InvalidResponse,
                        };
                        self.commits
                            .commit_failure(source.id, source.config_version, failure)
                            .await?;
                        return Err(failure);
                    }
                };
                let ignored_item_count = parsed.ignored_item_count;
                let format = parsed.format;
                let drafts = parsed
                    .items
                    .into_iter()
                    .map(|item| {
                        let dedup_key = item.dedup_key;
                        let display_name = item
                            .title
                            .clone()
                            .unwrap_or_else(|| "RSS automation item".to_owned());
                        AutomationEventDraft {
                            dedup_key,
                            downloader_connection_id: source.downloader_connection_id,
                            display_name,
                            source: item.into_source(),
                        }
                    })
                    .collect();
                let accepted = self
                    .commits
                    .commit(
                        source.id,
                        source.config_version,
                        FeedPollCommit {
                            cursor,
                            format: Some(format),
                            ignored_item_count,
                            drafts,
                        },
                    )
                    .await?;
                Ok(FeedPollResult {
                    accepted_event_count: accepted,
                    ignored_item_count,
                    not_modified: false,
                })
            }
        }
    }
}
