package mediaflow.client.core.events

import kotlinx.datetime.Instant
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject

/**
 * 手写 SSE 事件信封（消费 `/api/v1/events`）。
 *
 * 为什么不用生成的 `mediaflow.api.models.TaskEventEnvelope`：生成器把契约的
 * 16 变体 oneOf 压平成单一信封且 payload 固定为最后一个变体类型，无法承载
 * 联合语义。本信封以 `type` 判别 + [payload] 保留原始 JSON，消费端按需经
 * [decodePayload] 做类型化解码（未知事件类型原样保留，前向兼容）。
 *
 * `id` 即事件游标：SSE `Last-Event-ID` 续传与去重都以它为准（对齐 Web 端）。
 */
@Serializable
data class TaskEventEnvelope(
    @SerialName("id") val id: Long,
    @SerialName("type") val type: String,
    @SerialName("schema_version") val schemaVersion: String? = null,
    @SerialName("occurred_at") val occurredAt: Instant,
    @SerialName("task_id") val taskId: String? = null,
    @SerialName("payload") val payload: JsonObject,
) {
    /** 按需类型化解码 payload 变体；失败返回 null（未知形状不致命，保留原样语义）。 */
    inline fun <reified T> decodePayload(
        json: Json,
        deserializer: KSerializer<T>,
    ): T? = runCatching { json.decodeFromJsonElement(deserializer, payload) }.getOrNull()

    companion object {
        /** 契约 v1 事件类型（`contracts/events/v1/task-event.schema.json` 枚举）。 */
        const val TYPE_TASK_PROGRESS = "task.progress"
        const val TYPE_TASK_STATE_CHANGED = "task.state-changed"
        const val TYPE_PROCESSING_TASK_STATE_CHANGED = "processing-task.state-changed"
        const val TYPE_PROCESSING_TASK_IDENTIFICATION_DECIDED = "processing-task.identification-decided"
        const val TYPE_INBOX_DISCOVERY_HEALTH_CHANGED = "inbox.discovery-health-changed"
        const val TYPE_INTEGRATION_HEALTH_CHANGED = "integration.health-changed"
        const val TYPE_TASK_DECISION_ACCEPTED = "task-decision.accepted"
        const val TYPE_REVIEW_CASE_UPDATED = "review-case.updated"
        const val TYPE_CATALOG_MEDIA_CHANGED = "catalog.media-changed"
        const val TYPE_DOWNLOAD_TASK_CHANGED = "download-task.changed"
        const val TYPE_ORGANIZATION_TARGET_CHANGED = "organization-target.changed"
        const val TYPE_ORGANIZATION_RESULT_CHANGED = "organization-result.changed"
        const val TYPE_AUTOMATION_SOURCE_CHANGED = "automation-source.changed"
        const val TYPE_AUTOMATION_EVENT_CHANGED = "automation-event.changed"
        const val TYPE_IDENTIFICATION_ENHANCER_CHANGED = "identification-enhancer.changed"
        const val TYPE_STREAM_GAP = "stream.gap"
    }
}
