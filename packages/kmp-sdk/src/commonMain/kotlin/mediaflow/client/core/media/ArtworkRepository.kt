package mediaflow.client.core.media

import io.ktor.client.HttpClient
import io.ktor.client.request.prepareGet
import io.ktor.client.statement.bodyAsChannel
import io.ktor.http.HttpHeaders
import io.ktor.http.HttpStatusCode
import io.ktor.utils.io.ByteReadChannel
import io.ktor.utils.io.cancel
import io.ktor.utils.io.readAvailable
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.io.IOException
import kotlinx.serialization.json.Json
import mediaflow.client.core.SdkResult
import mediaflow.client.core.error.ErrorMapping
import mediaflow.client.core.error.MediaFlowError

/** 已授权海报字节；最大2MiB，解码和平台内存缓存由消费者持有。 */
data class Artwork(
    val bytes: ByteArray,
    val contentType: String,
)

/** 按opaque媒体ID读取Core海报；不接受远程图片URL，流式限制字节数并关闭响应。 */
class ArtworkRepository(
    private val client: HttpClient,
    private val json: Json,
) {
    /** 缺图/未授权保留HTTP错误，取消传播；无Content-Length时同样限制响应大小。 */
    @Suppress("TooGenericExceptionCaught", "SwallowedException") // 传输错误统一映射；仅错误信封IO失败回退已知HTTP状态，取消不吞。
    suspend fun poster(mediaItemId: String): SdkResult<Artwork> {
        if (!MEDIA_ID.matches(mediaItemId)) return invalid("invalid media identity")
        return try {
            client.prepareGet("/api/v1/media-items/$mediaItemId/artwork").execute { response ->
                val channel = response.bodyAsChannel()
                try {
                    if (response.status != HttpStatusCode.OK) {
                        val bytes =
                            try {
                                readBounded(channel, MAX_ERROR_BYTES)
                            } catch (_: IOException) {
                                currentCoroutineContext().ensureActive()
                                null
                            }
                        val envelope = bytes?.let { ErrorMapping.fromEnvelopeJson(it.decodeToString(), json) }
                        val error =
                            envelope?.copy(status = response.status.value)
                                ?: MediaFlowError.Http(
                                    response.status.value,
                                    null,
                                    "HTTP ${response.status.value}: ${response.status.description}",
                                    null,
                                )
                        return@execute SdkResult.failure(error)
                    }
                    val type =
                        response.headers[HttpHeaders.ContentType]
                            ?.substringBefore(';')
                            ?.trim()
                            ?.lowercase()
                    if (type !in IMAGE_TYPES) return@execute invalid("unsupported artwork content type")
                    val length = response.headers[HttpHeaders.ContentLength]?.toLongOrNull()
                    if (length != null && (length <= 0 || length > MAX_BYTES)) return@execute invalid("artwork exceeds size limit")
                    val bytes = readBounded(channel) ?: return@execute invalid("empty or oversized artwork")
                    SdkResult.success(Artwork(bytes, requireNotNull(type)))
                } finally {
                    channel.cancel()
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            SdkResult.failure(ErrorMapping.fromThrowable(failure))
        }
    }

    private suspend fun readBounded(
        channel: ByteReadChannel,
        maxBytes: Int = MAX_BYTES,
    ): ByteArray? {
        channel.closedCause?.let { throw it }
        currentCoroutineContext().ensureActive()
        val buffer = ByteArray(BUFFER_BYTES)
        val chunks = mutableListOf<ByteArray>()
        var total = 0
        var count = channel.readAvailable(buffer, 0, buffer.size)
        while (count >= 0) {
            if (count > 0) {
                total += count
                if (total > maxBytes) return null
                chunks.add(buffer.copyOf(count))
            }
            count = channel.readAvailable(buffer, 0, buffer.size)
        }
        channel.closedCause?.let { throw it }
        currentCoroutineContext().ensureActive()
        val bytes = ByteArray(total)
        var offset = 0
        chunks.forEach { chunk ->
            chunk.copyInto(bytes, offset)
            offset += chunk.size
        }
        return bytes.takeIf { it.isNotEmpty() }
    }

    private fun invalid(reason: String) = SdkResult.failure(MediaFlowError.Unexpected(reason))

    private companion object {
        const val MAX_BYTES = 2 * 1024 * 1024
        const val BUFFER_BYTES = 8192
        const val MAX_ERROR_BYTES = 64 * 1024
        val IMAGE_TYPES = setOf("image/jpeg", "image/png", "image/webp")
        val MEDIA_ID = Regex("[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
    }
}
