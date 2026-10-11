package mediaflow.client

import kotlinx.serialization.json.Json
import mediaflow.api.models.ApiErrorBody
import mediaflow.api.models.AutomationEvent
import mediaflow.api.models.AutomationEventPage
import mediaflow.api.models.AutomationSource
import mediaflow.api.models.AutomationSourcePage
import mediaflow.api.models.BootstrapRequest
import mediaflow.api.models.BootstrapStatusResponse
import mediaflow.api.models.DeploymentRootList
import mediaflow.api.models.DiscoveryPolicy
import mediaflow.api.models.DownloadTask
import mediaflow.api.models.DownloadTaskPage
import mediaflow.api.models.DownloaderConnection
import mediaflow.api.models.DownloaderConnectionPage
import mediaflow.api.models.IdentificationDetail
import mediaflow.api.models.IdentificationEnhancer
import mediaflow.api.models.InboxDirectory
import mediaflow.api.models.JellyfinIntegration
import mediaflow.api.models.MediaItemDetail
import mediaflow.api.models.MediaItemPage
import mediaflow.api.models.OrganizationTarget
import mediaflow.api.models.OrganizationTargetPage
import mediaflow.api.models.OrganizationTargetPreflight
import mediaflow.api.models.ProcessingTask
import mediaflow.api.models.ProcessingTaskPage
import mediaflow.api.models.ReviewCandidatePage
import mediaflow.api.models.ReviewCase
import mediaflow.api.models.ReviewCasePage
import mediaflow.api.models.ScanTask
import mediaflow.api.models.TaskDecisionReceipt
import mediaflow.api.models.TmdbIntegration
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * 契约 fixtures 回放：`contracts/examples/v1/`（AJV 已验证的 golden 样例）逐个
 * 解码进生成的 DTO——契约漂移防线（决策 D-0002）。仅 JVM 执行即可（漂移检测
 * 不依赖平台差异）；文件经相对路径读取。
 */
class ContractFixturesReplayTest {
    private val json = Json { ignoreUnknownKeys = true }
    private val fixturesDir: File =
        File(System.getProperty("user.dir"))
            .resolve("../../contracts/examples/v1")

    private fun fixture(name: String): String = fixturesDir.resolve("$name.json").readText()

    private inline fun <reified T> assertDecodes(name: String) {
        val decoded = json.decodeFromString<T>(fixture(name))
        assertTrue(decoded != null, "fixture $name must decode")
    }

    @Test
    fun identityAndSession() {
        assertDecodes<BootstrapStatusResponse>("identity-bootstrap-status")
        json.decodeFromString(
            mediaflow.api.models.BootstrapRequest
                .serializer(),
            fixture("identity-bootstrap-request"),
        )
        json.decodeFromString(
            mediaflow.api.models.SessionResponse
                .serializer(),
            fixture("identity-session"),
        )
    }

    @Test
    fun mediaWallAndDetail() {
        val page = json.decodeFromString(MediaItemPage.serializer(), fixture("media-item-page"))
        assertTrue(page.items.isNotEmpty(), "media-item-page fixture carries items")
        json.decodeFromString(MediaItemDetail.serializer(), fixture("media-item-detail"))
    }

    @Test
    fun reviewAndDecisions() {
        json.decodeFromString(ReviewCasePage.serializer(), fixture("review-case-page"))
        json.decodeFromString(ReviewCase.serializer(), fixture("review-case"))
        json.decodeFromString(ReviewCandidatePage.serializer(), fixture("review-candidate-page"))
        json.decodeFromString(TaskDecisionReceipt.serializer(), fixture("task-decision-receipt"))
        // 决定请求是"平铺 kind 判别"的 oneOf：生成的 sealed Request 走类判别器，
        // 无法解码平铺形状（决策 D-0007：SDK 手写决定命令承担该端点）。
        // 回放退为结构断言：三种 kind 的形状保持可解析 JSON 且 kind 值符合契约。
        listOf(
            "review-decision-generic-video-request" to "select-generic-video",
            "review-decision-rematch-request" to "rematch-with-hints",
            "review-decision-select-candidate-request" to "select-provider-candidate",
        ).forEach { (name, kind) ->
            val element = json.parseToJsonElement(fixture(name)).let { it as kotlinx.serialization.json.JsonObject }
            assertEquals(kind, (element["kind"] as kotlinx.serialization.json.JsonPrimitive).content, name)
        }
    }

    @Test
    fun processingTasksAndOrganization() {
        json.decodeFromString(ProcessingTask.serializer(), fixture("processing-task"))
        json.decodeFromString(ProcessingTaskPage.serializer(), fixture("processing-task-page"))
        json.decodeFromString(
            mediaflow.api.models.ProcessingTaskOrganization
                .serializer(),
            fixture("processing-task-organization"),
        )
        json.decodeFromString(OrganizationTarget.serializer(), fixture("organization-target"))
        json.decodeFromString(OrganizationTargetPage.serializer(), fixture("organization-target-page"))
        json.decodeFromString(OrganizationTargetPreflight.serializer(), fixture("organization-target-preflight"))
    }

    @Test
    fun scanningAndInbox() {
        json.decodeFromString(ScanTask.serializer(), fixture("scan-task"))
        json.decodeFromString(InboxDirectory.serializer(), fixture("inbox-directory"))
        json.decodeFromString(DeploymentRootList.serializer(), fixture("inbox-root-list"))
        json.decodeFromString(
            mediaflow.api.models.InboxDirectoryPreflight
                .serializer(),
            fixture("inbox-preflight"),
        )
        json.decodeFromString(DiscoveryPolicy.serializer(), fixture("discovery-policy"))
        json.decodeFromString(
            mediaflow.api.models.PutDiscoveryPolicyRequest
                .serializer(),
            fixture("discovery-policy-request"),
        )
    }

    @Test
    fun integrations() {
        json.decodeFromString(TmdbIntegration.serializer(), fixture("tmdb-integration"))
        json.decodeFromString(
            mediaflow.api.models.PutTmdbIntegrationRequest
                .serializer(),
            fixture("tmdb-integration-request"),
        )
        json.decodeFromString(JellyfinIntegration.serializer(), fixture("jellyfin-integration"))
        json.decodeFromString(IdentificationEnhancer.serializer(), fixture("identification-enhancer"))
        json.decodeFromString(IdentificationDetail.serializer(), fixture("identification-detail"))
    }

    @Test
    fun notificationsAndDownloads() {
        // M4 通知中心暂无 examples fixtures（通知 DTO 回放待主线补样例后接入）。
        json.decodeFromString(DownloaderConnection.serializer(), fixture("downloader-connection"))
        json.decodeFromString(DownloaderConnectionPage.serializer(), fixture("downloader-connection-page"))
        json.decodeFromString(DownloadTask.serializer(), fixture("download-task"))
        json.decodeFromString(DownloadTaskPage.serializer(), fixture("download-task-page"))
    }

    @Test
    fun automationSourcesAndEvents() {
        json.decodeFromString(AutomationSource.serializer(), fixture("automation-source"))
        json.decodeFromString(AutomationSourcePage.serializer(), fixture("automation-source-page"))
        json.decodeFromString(AutomationEvent.serializer(), fixture("automation-event"))
        json.decodeFromString(AutomationEventPage.serializer(), fixture("automation-event-page"))
    }

    @Test
    fun errorEnvelopeReplay() {
        // 错误信封 fixture 不在 examples 内；用契约点分错误码验证生成枚举覆盖。
        val body =
            json.decodeFromString(
                ApiErrorBody.serializer(),
                """{"error":{"code":"validation.failed","message":"limit out of range","request_id":"req-42"}}""",
            )
        assertEquals("validation.failed", body.error.code?.value)
    }

    @Test
    fun everySseEventFixtureDecodesThroughHandWrittenEnvelope() {
        // SSE 事件走 SDK 手写 envelope（决策 D-0006：生成 oneOf 被压平不可用）。
        mediaflow.client.core.events.TaskEventEnvelopeFixtures
            .assertAllDecode(fixturesDir, json)
    }
}
