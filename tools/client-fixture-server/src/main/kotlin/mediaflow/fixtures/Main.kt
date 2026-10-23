package mediaflow.fixtures

import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.server.application.call
import io.ktor.server.cio.CIO
import io.ktor.server.engine.embeddedServer
import io.ktor.server.request.receiveText
import io.ktor.server.response.header
import io.ktor.server.response.respondText
import io.ktor.server.response.respondTextWriter
import io.ktor.server.routing.delete
import io.ktor.server.routing.get
import io.ktor.server.routing.post
import io.ktor.server.routing.routing
import kotlinx.coroutines.delay
import java.io.File

/**
 * 契约 fixtures 本地服务器：把 `contracts/examples/v1/` 的 golden 样例按 REST/SSE
 * 形状吐出，供 Android 模拟器与 iOS 模拟器走通六屏（M5 第一批验收工具）。
 *
 * 用法：`just client-fixture-server`（默认 0.0.0.0:8642）。
 * 凭据：任意非空用户名 + 密码 `fixture`（其余 401）。
 * 设备通道（第二批）：enrollment 接受任意 8 位配对码或密码 `fixture`，返回固定
 * `mfdev_` 样例令牌；轮换/自吊销恒成功。
 */
fun main() {
    val port = System.getenv("FIXTURE_PORT")?.toIntOrNull() ?: 8642
    val fixtures =
        listOf("../../contracts/examples/v1", "../contracts/examples/v1", "contracts/examples/v1")
            .map(::File)
            .firstOrNull { it.isDirectory }
            ?: error("contracts/examples/v1 not found; run from the repo root via just client-fixture-server")

    fun fixture(name: String): String = fixtures.resolve("$name.json").readText()

    embeddedServer(CIO, port = port, host = "0.0.0.0") {
        routing {
            post("/api/v1/sessions") {
                // 简化登录：body 含 "password":"fixture" 即通过（不做 JSON 解析依赖）。
                val body = runCatching { call.receiveText() }.getOrNull() ?: ""
                if (body.contains("fixture")) {
                    call.response.header(
                        "Set-Cookie",
                        "__Host-mediaflow_session=fixture-session-value; Secure; HttpOnly; SameSite=Strict; Path=/",
                    )
                    call.respondText(fixture("identity-session"), ContentType.Application.Json, HttpStatusCode.Created)
                } else {
                    call.respondText(
                        """{"error":{"code":"auth.invalid_credentials","message":"bad credentials","request_id":"fx-1"}}""",
                        ContentType.Application.Json,
                        HttpStatusCode.Unauthorized,
                    )
                }
            }
            post("/api/v1/devices/enrollment") {
                // 设备 enrollment（第二批）：配对码模式接受任意非空 8 位码；
                // 凭据模式与其他端点同口径（密码 fixture）。令牌为固定样例值。
                val body = runCatching { call.receiveText() }.getOrNull() ?: ""
                val pairingMode = body.contains("pairing_code")
                val emptyCode =
                    body.contains("\"pairing_code\":\"\"") || body.contains("\"pairing_code\":null")
                val ok = if (pairingMode) !emptyCode else body.contains("fixture")
                if (ok) {
                    call.respondText(fixture("device-enrollment"), ContentType.Application.Json, HttpStatusCode.Created)
                } else {
                    call.respondText(
                        """{"error":{"code":"auth.invalid_credentials","message":"bad credentials","request_id":"fx-2"}}""",
                        ContentType.Application.Json,
                        HttpStatusCode.Unauthorized,
                    )
                }
            }
            post("/api/v1/devices/current/token-rotation") {
                call.respondText(fixture("device-token-rotation"), ContentType.Application.Json)
            }
            delete("/api/v1/devices/{id}") {
                call.respondText("", status = HttpStatusCode.NoContent)
            }
            get("/api/v1/session") {
                call.respondText(fixture("identity-session"), ContentType.Application.Json)
            }
            delete("/api/v1/session") {
                call.respondText("", status = HttpStatusCode.NoContent)
            }
            get("/api/v1/media-items") {
                val cursor = call.request.queryParameters["cursor"]
                if (cursor.isNullOrBlank()) {
                    call.respondText(fixture("media-item-page"), ContentType.Application.Json)
                } else {
                    // 第二页起返回末页（游标终结）。
                    call.respondText("""{"items":[],"next_cursor":null}""", ContentType.Application.Json)
                }
            }
            get("/api/v1/media-items/{id}") {
                call.respondText(fixture("media-item-detail"), ContentType.Application.Json)
            }
            get("/api/v1/review-cases") {
                call.respondText(fixture("review-case-page"), ContentType.Application.Json)
            }
            get("/api/v1/review-cases/{id}") {
                call.respondText(fixture("review-case"), ContentType.Application.Json)
            }
            get("/api/v1/review-cases/{id}/candidates") {
                call.respondText(fixture("review-candidate-page"), ContentType.Application.Json)
            }
            post("/api/v1/review-cases/{id}/decisions") {
                call.respondText(fixture("task-decision-receipt"), ContentType.Application.Json, HttpStatusCode.Accepted)
            }
            get("/api/v1/processing-tasks") {
                call.respondText(fixture("processing-task-page"), ContentType.Application.Json)
            }
            post("/api/v1/processing-tasks/{id}/cancel") {
                call.respondText(fixture("processing-task"), ContentType.Application.Json)
            }
            get("/api/v1/integrations/jellyfin") {
                call.respondText(fixture("jellyfin-integration"), ContentType.Application.Json)
            }
            get("/api/v1/events") {
                call.respondTextWriter(contentType = ContentType.Text.EventStream) {
                    var id = 1L
                    val events =
                        listOf(
                            "event-task-progress",
                            "event-catalog-media-changed",
                            "event-review-case-updated",
                            "event-processing-task-state-changed",
                        )
                    while (true) {
                        val payload = fixture(events[(id % events.size).toInt()])
                        write("id: $id\ndata: $payload\n\n")
                        flush()
                        id += 1
                        delay(5_000)
                    }
                }
            }
        }
    }.start(wait = true)
}
