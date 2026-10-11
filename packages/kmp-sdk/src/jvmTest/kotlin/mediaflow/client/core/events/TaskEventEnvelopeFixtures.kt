package mediaflow.client.core.events

import kotlinx.serialization.json.Json
import java.io.File
import kotlin.test.assertTrue

/** 事件 fixtures 断言（JVM 文件 IO）：全部 event-*.json 经手写信封解码并校验不变量。 */
object TaskEventEnvelopeFixtures {
    fun assertAllDecode(
        fixturesDir: File,
        json: Json,
    ) {
        val files =
            fixturesDir
                .listFiles { file -> file.name.startsWith("event-") && file.name.endsWith(".json") }
                .orEmpty()
                .sortedBy { it.name }
        assertTrue(files.isNotEmpty(), "expected SSE event fixtures under ${fixturesDir.absolutePath}")
        for (file in files) {
            val event = json.decodeFromString(TaskEventEnvelope.serializer(), file.readText())
            assertTrue(event.id >= 1, "${file.name}: id must be a positive cursor")
            assertTrue(event.type.isNotBlank(), "${file.name}: type must be present")
            if (event.type == TaskEventEnvelope.TYPE_STREAM_GAP) {
                assertTrue(event.taskId == null, "${file.name}: stream.gap carries no task_id")
            }
        }
    }
}
