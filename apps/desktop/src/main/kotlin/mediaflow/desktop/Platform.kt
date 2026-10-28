package mediaflow.desktop

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import mediaflow.client.core.auth.DeviceAuthStore
import mediaflow.client.core.auth.StoredCredentials
import mediaflow.client.core.net.MediaFlowApiFactory
import java.awt.Desktop
import java.net.URI
import java.nio.file.Path
import java.security.MessageDigest

fun normalizeServer(input: String): String {
    val uri = URI(input.trim())
    require(uri.scheme?.lowercase() in setOf("http", "https") && uri.host != null) { "请输入有效的 HTTP/HTTPS 服务器地址" }
    require(uri.userInfo == null && uri.rawQuery == null && uri.rawFragment == null && uri.path.orEmpty() in setOf("", "/")) {
        "服务器地址不能包含账号、路径、查询或片段"
    }
    val scheme = uri.scheme.lowercase()
    val port = uri.port.takeUnless { it == -1 || (scheme == "https" && it == 443) || (scheme == "http" && it == 80) } ?: -1
    require(port == -1 || port in 1..65535) { "端口无效" }
    return URI(scheme, null, uri.host.lowercase(), port, null, null, null).toASCIIString()
}

fun cacheIdentity(
    server: String,
    device: String,
): String =
    MessageDigest
        .getInstance("SHA-256")
        .digest("$server\u0000$device".toByteArray())
        .joinToString("") { "%02x".format(it) }

fun appDirectory(): Path {
    val home = System.getProperty("user.home")
    return when {
        System.getProperty("os.name").startsWith("Mac") -> Path.of(home, "Library", "Application Support", "MediaFlow")
        System
            .getProperty(
                "os.name",
            ).startsWith("Windows") -> Path.of(System.getenv("LOCALAPPDATA") ?: error("LOCALAPPDATA unavailable"), "MediaFlow")
        else -> error("当前仅支持 macOS 和 Windows")
    }
}

interface CredentialVault {
    fun read(): ByteArray?

    fun write(value: ByteArray)

    fun delete()
}

class VaultAuthStore(
    private val vault: CredentialVault,
) : DeviceAuthStore {
    private val json = MediaFlowApiFactory.defaultJson()

    override suspend fun read(): StoredCredentials? =
        withContext(Dispatchers.IO) {
            vault.read()?.let { bytes ->
                try {
                    json.decodeFromString(StoredCredentials.serializer(), bytes.toString(Charsets.UTF_8))
                } finally {
                    bytes.fill(0)
                }
            }
        }

    override suspend fun save(credentials: StoredCredentials) =
        withContext(Dispatchers.IO) {
            val bytes = json.encodeToString(StoredCredentials.serializer(), credentials).toByteArray()
            try {
                vault.write(bytes)
            } finally {
                bytes.fill(0)
            }
        }

    override suspend fun clear() = withContext(Dispatchers.IO) { vault.delete() }
}

fun secureStore(server: String): DeviceAuthStore {
    val target = "MediaFlow/desktop/${cacheIdentity(server, "credential")}"
    return VaultAuthStore(
        when {
            System.getProperty("os.name").startsWith("Mac") -> MacKeychain(target)
            System.getProperty("os.name").startsWith("Windows") -> WindowsCredentials(target)
            else -> error("当前系统没有受支持的安全凭据存储")
        },
    )
}

fun openExternal(url: String) {
    val uri = URI(url)
    require(uri.scheme?.lowercase() in setOf("https", "http") && uri.host != null && uri.userInfo == null) { "外部地址无效" }
    check(Desktop.isDesktopSupported() && Desktop.getDesktop().isSupported(Desktop.Action.BROWSE)) { "系统不支持打开外部链接" }
    Desktop.getDesktop().browse(uri)
}
