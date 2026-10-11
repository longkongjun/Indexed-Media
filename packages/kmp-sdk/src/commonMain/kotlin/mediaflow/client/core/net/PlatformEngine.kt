package mediaflow.client.core.net

import io.ktor.client.engine.HttpClientEngine

/**
 * 平台 HTTP 引擎边界（expect/actual 仅用于平台能力边界，不进业务层）：
 * JVM 用 CIO（纯 JVM 无本地依赖）、Android 用 OkHttp（连接池/HTTP2 实战成熟）、
 * iOS 用 Darwin（URLSession，自动系统代理与省电）。
 */
expect fun platformHttpClientEngine(): HttpClientEngine
