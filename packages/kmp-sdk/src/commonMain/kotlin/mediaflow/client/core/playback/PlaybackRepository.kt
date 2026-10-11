package mediaflow.client.core.playback

import kotlinx.serialization.json.Json
import mediaflow.api.apis.DefaultApi
import mediaflow.api.models.JellyfinIntegration
import mediaflow.client.core.SdkResult
import mediaflow.client.core.apiCall

/** 外部播放跳转目标：Jellyfin 独立连接的脱敏投影 + 可跳转地址推导。 */
class PlaybackRepository(
    private val api: DefaultApi,
    private val json: Json,
) {
    /** Jellyfin 连接投影（脱敏；未配置时 configured=false）。 */
    suspend fun jellyfin(): SdkResult<JellyfinIntegration> = apiCall(json) { api.getJellyfinIntegration() }

    /**
     * 可跳转的播放首页地址：仅当已配置且健康（healthy）时返回 base_url；其余
     * 情况返回 null（UI 展示"外部播放不可用"并给出去配置的引导）。
     * 契约语义：Jellyfin 独立于 MediaFlow 库，绝不是本地任务的成功条件。
     */
    suspend fun externalPlaybackUrl(): SdkResult<String?> =
        when (val projection = jellyfin()) {
            is SdkResult.Success -> {
                val integration = projection.value
                val url =
                    integration.baseUrl.takeIf {
                        integration.configured &&
                            !it.isNullOrEmpty() &&
                            integration.health == JellyfinIntegration.Health.healthy
                    }
                SdkResult.success(url)
            }
            is SdkResult.Failure -> projection
        }
}
