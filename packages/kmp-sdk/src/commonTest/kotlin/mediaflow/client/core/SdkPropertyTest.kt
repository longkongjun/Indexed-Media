package mediaflow.client.core

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import mediaflow.api.models.SelectGenericVideoCommand
import mediaflow.api.models.SelectProviderCandidateCommand
import mediaflow.client.core.auth.AuthRepository
import mediaflow.client.core.error.ErrorMapping
import mediaflow.client.core.error.MediaFlowError
import mediaflow.client.core.events.TaskEventEnvelope
import kotlin.random.Random
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * 属性测试（C1.9）：种子化随机（可复现）验证不变量，不引入属性测试框架依赖。
 * 覆盖：错误信封往返、重试分类一致性、SSE 信封解码保真、决定命令平铺编码、
 * cookie 解析。
 */
class SdkPropertyTest {
    private val json = Json { ignoreUnknownKeys = true }
    private val random = Random(seed = 20261011)

    private fun randomId(): Long = random.nextLong(1, 9_000_000_000_000L)

    private fun randomToken(length: Int = 24): String = buildString { repeat(length) { append(('a'..'z').random(random)) } }

    @Test
    fun errorEnvelopeRoundTripPreservesAllFields() {
        repeat(100) {
            val status = random.nextInt(400, 600)
            val code = "err.${randomToken(6)}"
            val message = randomToken(12)
            val requestId = randomId().toString()
            val error =
                ErrorMapping
                    .fromEnvelopeJson(
                        """{"error":{"code":"$code","message":"$message","request_id":"$requestId"}}""",
                        json,
                    )!!
                    .copy(status = status)
            assertEquals(code, error.code)
            assertEquals(message, error.message)
            assertEquals(requestId, error.requestId)
            assertEquals(status, error.status)
        }
    }

    @Test
    fun retryableClassificationIsStableUnderStatusSet() {
        repeat(100) {
            val status = random.nextInt(100, 600)
            val expected = status in MediaFlowError.RETRYABLE_STATUS
            assertEquals(expected, MediaFlowError.Http(status, null, "", null).retryable, "status=$status")
        }
    }

    @Test
    fun sseEnvelopeDecodeKeepsCursorTypeAndPayloadIntact() {
        repeat(100) {
            val id = randomId()
            val type = listOf("task.progress", "catalog.media-changed", "stream.gap").random(random)
            val taskId = if (random.nextBoolean()) "\"${randomId()}\"" else "null"
            val probeValue = randomId().toString()
            val payload = buildJsonObject { put("probe", probeValue) }
            val envelope =
                json.decodeFromString(
                    TaskEventEnvelope.serializer(),
                    """{"id":$id,"type":"$type","schema_version":"1",""" +
                        """"occurred_at":"2026-07-17T00:00:00Z","task_id":$taskId,""" +
                        """"payload":{"probe":"$probeValue"}}""",
                )
            assertEquals(id, envelope.id)
            assertEquals(type, envelope.type)
            assertEquals(payload, envelope.payload)
        }
    }

    @Test
    fun decisionCommandEncodesFlatWithoutWrapper() {
        repeat(100) {
            val providerId = randomId().toString()
            val title = randomToken(8)
            // 选择候选：kind 顶层平铺 + provider_id 保真（决策 D-0007 的形状不变量）。
            val candidate =
                SelectProviderCandidateCommand(
                    kind = "select-provider-candidate",
                    provider = "tmdb",
                    mediaType =
                        if (random.nextBoolean()) {
                            SelectProviderCandidateCommand.MediaType.movie
                        } else {
                            SelectProviderCandidateCommand.MediaType.tv
                        },
                    providerId = providerId,
                    saveFeedback = random.nextBoolean(),
                )
            val encoded = json.encodeToString(SelectProviderCandidateCommand.serializer(), candidate)
            val obj = json.parseToJsonElement(encoded).let { it as kotlinx.serialization.json.JsonObject }
            assertEquals("select-provider-candidate", (obj["kind"] as kotlinx.serialization.json.JsonPrimitive).content)
            assertEquals(providerId, (obj["provider_id"] as kotlinx.serialization.json.JsonPrimitive).content)
            assertTrue("type" !in obj.keys, "sealed 类判别包装不得出现")

            // 通用视频：display_title 保真。
            val generic =
                SelectGenericVideoCommand(
                    kind = "select-generic-video",
                    displayTitle = title,
                    groupHint = null,
                    saveGroupingFeedback = false,
                )
            val encodedGeneric = json.encodeToString(SelectGenericVideoCommand.serializer(), generic)
            val genericObj = json.parseToJsonElement(encodedGeneric).let { it as kotlinx.serialization.json.JsonObject }
            assertEquals(title, (genericObj["display_title"] as kotlinx.serialization.json.JsonPrimitive).content)
        }
    }

    @Test
    fun sessionCookieParsingExtractsValueAcrossAttributeOrders() {
        repeat(100) {
            val value = randomToken(32)
            val attributes = listOf("Secure", "HttpOnly", "SameSite=Strict", "Path=/").shuffled(random)
            val header = "__Host-mediaflow_session=$value; ${attributes.joinToString("; ")}"
            assertEquals(value, AuthRepository.parseSessionCookie(header))
            // 前缀不匹配或空值必须拒绝。
            assertEquals(null, AuthRepository.parseSessionCookie("other=$value; Secure"))
            assertEquals(null, AuthRepository.parseSessionCookie("__Host-mediaflow_session=; Secure"))
        }
    }
}
