package mediaflow.fixtures

import kotlinx.coroutines.runBlocking
import java.io.File
import java.net.HttpURLConnection
import java.net.URI
import kotlin.test.Test
import kotlin.test.assertEquals

class CancelContractTest {
    @Test
    fun cancellationReturnsTheAcceptedContractStatus() =
        runBlocking {
            val fixtures = File("../../contracts/examples/v1")
            val server = fixtureServer(fixtures, port = 0, host = "127.0.0.1").start(wait = false)
            try {
                val port =
                    server.engine
                        .resolvedConnectors()
                        .single()
                        .port
                val connection =
                    URI(
                        "http://127.0.0.1:$port/api/v1/processing-tasks/test-task/cancel",
                    ).toURL().openConnection() as HttpURLConnection
                try {
                    connection.requestMethod = "POST"
                    connection.connectTimeout = 5_000
                    connection.readTimeout = 5_000
                    assertEquals(202, connection.responseCode)
                    assertEquals(
                        fixtures.resolve("processing-task.json").readText(),
                        connection.inputStream.bufferedReader().use { it.readText() },
                    )
                } finally {
                    connection.disconnect()
                }
            } finally {
                server.stop(gracePeriodMillis = 0, timeoutMillis = 1_000)
            }
        }
}
