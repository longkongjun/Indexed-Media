package mediaflow.client.core.net

import io.ktor.client.engine.HttpClientEngine
import io.ktor.client.engine.darwin.Darwin

/** iOS：Darwin（URLSession，系统代理/省电/ATS 语义一致）。 */
actual fun platformHttpClientEngine(): HttpClientEngine = Darwin.create()
