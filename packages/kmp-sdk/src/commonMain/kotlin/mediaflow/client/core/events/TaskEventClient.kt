package mediaflow.client.core.events

import io.ktor.client.HttpClient
import io.ktor.client.engine.HttpClientEngine
import io.ktor.client.plugins.DefaultRequest
import io.ktor.client.plugins.HttpTimeout
import io.ktor.client.request.header
import io.ktor.client.request.prepareGet
import io.ktor.client.statement.bodyAsChannel
import io.ktor.http.ContentType
import io.ktor.http.HttpHeaders
import io.ktor.http.isSuccess
import io.ktor.utils.io.ByteReadChannel
import io.ktor.utils.io.readUTF8Line
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.isActive
import kotlinx.io.IOException
import kotlinx.serialization.json.Json
import mediaflow.client.core.net.AuthHeaderPlugin
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import kotlin.coroutines.coroutineContext

/** SSE 单行上限：防御异常服务端行为（超长行只可能来自故障注入）。 */
private const val MAX_LINE_LENGTH = 64 * 1024

/** SSE 连接状态（页面语义：Reconnecting 映射弱网/离线提示）。 */
enum class TaskEventConnectionState { Idle, Connecting, Connected, Reconnecting }

/**
 * `/api/v1/events` 的 SSE 消费端。
 *
 * 传输实现（决策 D-0008）：不走 ktor-sse 插件——其按引擎能力放行（MockEngine
 * 不支持响应往返、CIO 引擎缺能力声明），改为普通流式 GET + 手写 SSE 行解析，
 * 全平台引擎可用且可测。
 *
 * 语义对齐 Web 端（useProjectionEvents）：
 * - **游标续传**：每次（重）连接以 `Last-Event-ID` 头携带最后已接受事件 id；
 * - **严格单调去重**：id ≤ 最后已接受 id 的事件丢弃（重连补发窗口内天然幂等）；
 * - **指数退避重连**：断开后 1s 起步翻倍至 60s 封顶，收到有效事件即复位；
 * - **stream.gap 透传**：缺口事件作为普通信封上抛，由 Repository 层触发权威刷新。
 *
 * `cursor` 供宿主持久化（跨进程启动续传）；`connection` 供 UI 展示连接状态。
 * 帧内 `id:` 行忽略——游标以 JSON 信封的 `id` 字段为唯一真源。
 */
class TaskEventClient(
    baseUrl: String,
    authCache: SessionAuthCache,
    json: Json = MediaFlowApiFactory.defaultJson(),
    engine: HttpClientEngine,
    private val initialBackoffMs: Long = DEFAULT_INITIAL_BACKOFF_MS,
    private val maxBackoffMs: Long = DEFAULT_MAX_BACKOFF_MS,
) {
    private val decoder: Json = json

    private val httpClient =
        HttpClient(engine) {
            expectSuccess = false
            install(DefaultRequest) {
                url(baseUrl)
                header(HttpHeaders.Origin, baseUrl.removeSuffix("/"))
                header(MediaFlowApiFactory.SEC_FETCH_SITE_HEADER, "same-origin")
            }
            install(AuthHeaderPlugin.create(authCache))
            // SSE 是无限时长流：请求级超时关闭，仅保留连接建立超时。
            install(HttpTimeout) {
                requestTimeoutMillis = Long.MAX_VALUE
                connectTimeoutMillis = MediaFlowApiFactory.CONNECT_TIMEOUT_MS * 2
            }
        }

    /** 最后已接受事件 id（0 = 尚无事件）。宿主订阅持久化以支持冷启动续传。 */
    val cursor: MutableStateFlow<Long> = MutableStateFlow(0L)

    /** 连接状态；重连中即 Web 端 "reconnecting" 语义。 */
    val connection: MutableStateFlow<TaskEventConnectionState> = MutableStateFlow(TaskEventConnectionState.Idle)

    /**
     * 事件流：从 [resumeFrom] 之后继续，永不正常完成（取消协程即断开）。
     * 无效帧（解析失败）跳过不中断流。
     *
     * Suppress 说明：重连循环的 catch(Exception) 是指定韧性点——任何传输错误只
     * 降级为连接状态并退避重试，不允许终结流；异常可经连接状态与游标观测。
     */
    @Suppress("TooGenericExceptionCaught", "SwallowedException")
    fun events(resumeFrom: Long = cursor.value): Flow<TaskEventEnvelope> =
        flow {
            var lastAccepted = resumeFrom
            var backoffMs = initialBackoffMs
            while (coroutineContext.isActive) {
                try {
                    connection.value = TaskEventConnectionState.Connecting
                    httpClient
                        .prepareGet("/api/v1/events") {
                            header(HttpHeaders.Accept, ContentType.Text.EventStream.toString())
                            if (lastAccepted > 0) header("Last-Event-ID", lastAccepted.toString())
                        }.execute { response ->
                            if (!response.status.isSuccess()) {
                                throw IOException("SSE stream failed with HTTP ${response.status.value}")
                            }
                            connection.value = TaskEventConnectionState.Connected
                            readFrames(response.bodyAsChannel()) { data ->
                                val envelope =
                                    runCatching { decoder.decodeFromString(TaskEventEnvelope.serializer(), data) }
                                        .getOrNull() ?: return@readFrames
                                if (envelope.id <= lastAccepted) return@readFrames
                                lastAccepted = envelope.id
                                cursor.value = lastAccepted
                                backoffMs = initialBackoffMs
                                emit(envelope)
                            }
                        }
                    // 会话被对端正常结束：按断线处理进入重连。
                    connection.value = TaskEventConnectionState.Reconnecting
                } catch (cancellation: CancellationException) {
                    throw cancellation
                } catch (failure: Exception) {
                    connection.value = TaskEventConnectionState.Reconnecting
                }
                delay(backoffMs)
                backoffMs = (backoffMs * 2).coerceAtMost(maxBackoffMs)
            }
        }

    fun close() {
        httpClient.close()
    }

    companion object {
        const val DEFAULT_INITIAL_BACKOFF_MS = 1_000L
        const val DEFAULT_MAX_BACKOFF_MS = 60_000L
    }
}

/**
 * SSE 行解析：空行分发事件；只消费 `data:` 行（多行拼接），`id:`/`event:`/`retry:`
 * 与注释行（`;` 开头）按游标真源与透传原则忽略。
 */
private suspend fun readFrames(
    channel: ByteReadChannel,
    onEvent: suspend (String) -> Unit,
) {
    val data = StringBuilder()
    while (true) {
        val line = channel.readUTF8Line(MAX_LINE_LENGTH) ?: break
        when {
            line.isEmpty() -> {
                if (data.isNotEmpty()) {
                    onEvent(data.toString())
                    data.setLength(0)
                }
            }
            line.startsWith("data:") -> {
                val value = line.removePrefix("data:")
                if (data.isNotEmpty()) data.append('\n')
                data.append(if (value.startsWith(" ")) value.drop(1) else value)
            }
            else -> Unit
        }
    }
    if (data.isNotEmpty()) onEvent(data.toString())
}
