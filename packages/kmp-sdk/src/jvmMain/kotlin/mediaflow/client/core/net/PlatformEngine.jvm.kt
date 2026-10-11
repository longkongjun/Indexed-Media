package mediaflow.client.core.net

import io.ktor.client.engine.HttpClientEngine
import io.ktor.client.engine.cio.CIO

/** JVM：CIO（纯 JVM 实现，无本地依赖；测试与桌面场景稳定）。 */
actual fun platformHttpClientEngine(): HttpClientEngine = CIO.create()
