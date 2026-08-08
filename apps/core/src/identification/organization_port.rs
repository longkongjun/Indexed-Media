use async_trait::async_trait;
use sqlx::{Row as _, SqlitePool};
use uuid::Uuid;

use crate::organization::model::{ConfirmedNfoMetadata, ConfirmedProviderId, NfoProvider};
use crate::shared::error::AppError;
use crate::shared::error::ErrorCode;

use super::manual::model::ManualDecisionInput;

/// organization planner 可消费、且只包含已核对身份的窄投影。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmedOrganizationIdentity {
    /// 已核对电影身份。
    Movie {
        /// 规范标题。
        title: String,
        /// 可选发行年份。
        year: Option<u16>,
        /// 可选物理版本标签。
        version_label: Option<String>,
        /// 被选中的候选 ID。
        candidate_id: Uuid,
        /// 只包含已核对字段的 NFO 补充元数据。
        nfo_metadata: ConfirmedNfoMetadata,
    },
    /// 已核对剧集文件身份。
    SeriesEpisode {
        /// 剧集标题。
        series_title: String,
        /// 季号。
        season: u16,
        /// 同一物理文件覆盖的集号。
        episodes: Vec<u16>,
        /// 可选物理版本标签。
        version_label: Option<String>,
        /// 被选中的候选 ID。
        candidate_id: Uuid,
        /// 只包含已核对字段的 NFO 补充元数据。
        nfo_metadata: ConfirmedNfoMetadata,
    },
    /// 管理员确认的受限通用视频意图。
    GenericVideo {
        /// 单项展示标题。
        title: String,
        /// 可选分组提示。
        group_hint: Option<String>,
        /// 人工决定 ID。
        decision_id: Uuid,
    },
}

#[async_trait]
/// identification 向 organization 暴露已核对当前身份的只读应用端口。
pub trait OrganizationIdentityPort: Send + Sync {
    /// 读取账户任务当前可用于规划的确认身份。
    ///
    /// # Errors
    ///
    /// 任务无确认身份、账户不匹配或持久化读取失败时返回稳定应用错误。
    async fn confirmed_identity(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<ConfirmedOrganizationIdentity, AppError>;
}

#[derive(Clone)]
/// 从 identification 私有表读取已提交确认结论的窄 `SQLite` 适配器。
pub struct SqliteOrganizationIdentityPort {
    pool: SqlitePool,
}

impl SqliteOrganizationIdentityPort {
    /// 使用已有连接池创建只读适配器。
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl OrganizationIdentityPort for SqliteOrganizationIdentityPort {
    async fn confirmed_identity(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<ConfirmedOrganizationIdentity, AppError> {
        if let Some(row) = sqlx::query(
            "SELECT d.id,d.payload_json
             FROM tasks_processing_tasks t
             JOIN identification_task_decisions d ON d.id=t.current_task_decision_id
             WHERE t.account_id=? AND t.id=? AND d.kind='select-generic-video'",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        {
            let input: ManualDecisionInput =
                serde_json::from_str(&row.get::<String, _>("payload_json")).map_err(internal)?;
            let ManualDecisionInput::SelectGenericVideo {
                display_title,
                group_hint,
                ..
            } = input
            else {
                return Err(invalid_identity());
            };
            return Ok(ConfirmedOrganizationIdentity::GenericVideo {
                title: display_title,
                group_hint,
                decision_id: uuid(&row, "id")?,
            });
        }

        let row = sqlx::query(
            "SELECT candidate.id,candidate.media_type,candidate.provider_id,candidate.year,
                    candidate.original_title,
                    (SELECT title.value FROM identification_candidate_titles title
                     WHERE title.candidate_id=candidate.id AND title.kind='title'
                     ORDER BY title.ordinal LIMIT 1) AS title
             FROM tasks_processing_tasks task
             JOIN identification_attempts attempt ON attempt.task_id=task.id
             JOIN identification_decisions decision ON decision.attempt_id=attempt.id
             JOIN identification_candidates candidate
               ON candidate.id=decision.selected_candidate_id
             WHERE task.account_id=? AND task.id=? AND decision.level='confirmed'
             ORDER BY attempt.ordinal DESC LIMIT 1",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(invalid_identity)?;
        let candidate_id = uuid(&row, "id")?;
        let title = row
            .get::<Option<String>, _>("title")
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(invalid_identity)?;
        let year = row
            .get::<Option<i64>, _>("year")
            .map(|value| u16::try_from(value).map_err(internal))
            .transpose()?;
        let nfo_metadata = ConfirmedNfoMetadata {
            original_title: row.get("original_title"),
            year,
            plot: None,
            provider_id: Some(ConfirmedProviderId {
                provider: NfoProvider::Tmdb,
                value: row.get::<i64, _>("provider_id").to_string(),
            }),
        };
        match row.get::<String, _>("media_type").as_str() {
            "movie" => Ok(ConfirmedOrganizationIdentity::Movie {
                title,
                year,
                version_label: None,
                candidate_id,
                nfo_metadata,
            }),
            "tv" => {
                let episodes = sqlx::query_as::<_, (i64, i64)>(
                    "SELECT season_number,episode_number
                     FROM identification_candidate_episodes
                     WHERE candidate_id=? ORDER BY season_number,episode_number",
                )
                .bind(candidate_id.as_bytes().as_slice())
                .fetch_all(&self.pool)
                .await
                .map_err(internal)?;
                let Some((first_season, _)) = episodes.first().copied() else {
                    return Err(invalid_identity());
                };
                if episodes.iter().any(|(season, _)| *season != first_season) {
                    return Err(invalid_identity());
                }
                Ok(ConfirmedOrganizationIdentity::SeriesEpisode {
                    series_title: title,
                    season: u16::try_from(first_season).map_err(internal)?,
                    episodes: episodes
                        .into_iter()
                        .map(|(_, episode)| u16::try_from(episode).map_err(internal))
                        .collect::<Result<_, _>>()?,
                    version_label: None,
                    candidate_id,
                    nfo_metadata,
                })
            }
            _ => Err(invalid_identity()),
        }
    }
}

fn uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Uuid, AppError> {
    Uuid::from_slice(&row.get::<Vec<u8>, _>(column)).map_err(internal)
}

fn invalid_identity() -> AppError {
    AppError::new(
        ErrorCode::TaskInvalidState,
        "processing task has no confirmed organization identity",
    )
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
