package mediaflow.client.core.net

import io.ktor.client.engine.HttpClientEngine
import io.ktor.client.engine.okhttp.OkHttp

/** Android：OkHttp（连接池、HTTP/2 与系统网络栈集成成熟）。 */
actual fun platformHttpClientEngine(): HttpClientEngine = OkHttp.create()
