package mediaflow.client.core.auth

import kotlinx.cinterop.CValuesRef
import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.alloc
import kotlinx.cinterop.memScoped
import kotlinx.cinterop.ptr
import kotlinx.cinterop.readBytes
import kotlinx.cinterop.toCValues
import kotlinx.cinterop.value
import kotlinx.serialization.json.Json
import platform.CoreFoundation.CFDataCreate
import platform.CoreFoundation.CFDictionaryAddValue
import platform.CoreFoundation.CFDictionaryCreateMutable
import platform.CoreFoundation.CFMutableDictionaryRef
import platform.CoreFoundation.CFRelease
import platform.CoreFoundation.CFStringCreateWithCString
import platform.CoreFoundation.CFStringRef
import platform.CoreFoundation.CFTypeRefVar
import platform.CoreFoundation.kCFBooleanTrue
import platform.Foundation.NSData
import platform.Security.SecItemAdd
import platform.Security.SecItemCopyMatching
import platform.Security.SecItemDelete
import platform.Security.errSecSuccess

/**
 * iOS Keychain 凭据存储：`kSecClassGenericPassword` 条目（service + account 定位），
 * `AfterFirstUnlockThisDeviceOnly` 可访问性——仅本设备、解锁后可读，不随备份迁移。
 *
 * 失读（条目缺失/钥匙串拒绝）按"无凭据"处理；写失败抛 [KeychainException]
 * 由调用方按致命错误呈现（凭据不落明文，宁可不进主界面）。
 */
@OptIn(ExperimentalForeignApi::class)
class KeychainDeviceAuthStore(
    private val service: String = "mediaflow.device-credentials",
    private val account: String = "default",
) : DeviceAuthStore {
    private val json = Json { ignoreUnknownKeys = true }

    override suspend fun read(): StoredCredentials? {
        val data = query() ?: return null
        return runCatching {
            json.decodeFromString(StoredCredentials.serializer(), data.decodeToString())
        }.getOrNull()
    }

    override suspend fun save(credentials: StoredCredentials) {
        val payload = json.encodeToString(StoredCredentials.serializer(), credentials).encodeToByteArray()
        // upsert：delete（幂等）+ add。绕开 query 判定与并发窗口——Keychain 无
        // 原生 upsert，先查后写在条目残留时以 -25299 撞重。
        delete()
        add(payload)
    }

    override suspend fun clear() {
        delete()
    }

    /** 非挂起的同步清理：供错误路径（非协程上下文，如 iOS bridge 的文案函数）
     * 即时吊销本地凭据；语义等同 [clear]。 */
    fun syncClear() {
        delete()
    }

    private fun query(): ByteArray? =
        memScoped {
            val owned = mutableListOf<CFStringRef>()
            val dictionary = CFDictionaryCreateMutable(null, 0, null, null)!!
            fillBase(dictionary, owned)
            val returnKey = cfstr(KEY_RETURN_DATA)
            owned.add(returnKey)
            CFDictionaryAddValue(dictionary, returnKey, cfTrue())
            val result = alloc<CFTypeRefVar>()
            val status = SecItemCopyMatching(dictionary, result.ptr)
            var bytes: ByteArray? = null
            val raw = result.value
            if (status == errSecSuccess && raw != null) {
                // CFData 与 NSData toll-free 桥接：经 ObjC 指针解释取字节。
                val data = kotlinx.cinterop.interpretObjCPointer<NSData>(raw.rawValue)
                bytes = data.bytes?.readBytes(data.length.toInt())
                CFRelease(raw)
            }
            CFRelease(dictionary)
            owned.forEach { CFRelease(it) }
            bytes
        }

    private fun add(payload: ByteArray): Unit =
        memScoped {
            val owned = mutableListOf<CFStringRef>()
            val dictionary = CFDictionaryCreateMutable(null, 0, null, null)!!
            fillBase(dictionary, owned)
            val valueKey = cfstr(KEY_VALUE_DATA)
            owned.add(valueKey)
            CFDictionaryAddValue(
                dictionary,
                valueKey,
                CFDataCreate(null, payload.toUByteArray().toCValues(), payload.size.toLong()),
            )
            // pdmn 省略：自建 CFString 的 accessible 值在 iOS 26 SecItemAdd 的值域
            // 校验被拒（-50，任意字符串值皆然；常量单例指针 interop 不可用）。省略后
            // 条目落到默认 kSecAttrAccessibleWhenUnlocked——严格于原设计的
            // AfterFirstUnlockThisDeviceOnly，锁屏时不可读，语义可接受。
            val status = SecItemAdd(dictionary, null)
            CFRelease(dictionary)
            owned.forEach { CFRelease(it) }
            if (status != errSecSuccess) throw KeychainException("keychain add failed: $status")
        }

    private fun delete(): Unit =
        memScoped {
            val owned = mutableListOf<CFStringRef>()
            val dictionary = CFDictionaryCreateMutable(null, 0, null, null)!!
            fillBase(dictionary, owned)
            SecItemDelete(dictionary)
            CFRelease(dictionary)
            owned.forEach { CFRelease(it) }
        }

    /**
     * 填入 service+account 定位键值（借用语义：字符串由调用方 [owned] 持有并在
     * SecItem 调用后统一释放）。键值一律自建 CFString——Kotlin interop 对 Security
     * 常量（kSecClass 等）与 CF 回调常量暴露的指针在运行时不可靠，常量字符串值
     * （"class"/"svce"/…）是公开稳定的 ABI，自建等价。
     */
    private fun fillBase(
        dictionary: CFMutableDictionaryRef,
        owned: MutableList<CFStringRef>,
    ) {
        for ((key, value) in listOf(KEY_CLASS to CLASS_GENERIC_PASSWORD, KEY_ATTR_SERVICE to service, KEY_ATTR_ACCOUNT to account)) {
            val keyRef = cfstr(key)
            val valueRef = cfstr(value)
            owned.addAll(listOf(keyRef, valueRef))
            CFDictionaryAddValue(dictionary, keyRef, valueRef)
        }
    }

    private fun cfstr(value: String): CFStringRef = CFStringCreateWithCString(null, value, 0u)!!

    /**
     * `kSecReturnData` 的布尔真值：直接用 CoreFoundation 的 `kCFBooleanTrue`
     * 单例（签名进程下指针稳定；plist 解析的等价物在跨作用域传递时行为不确定，
     * 是此前 query 判定失效的候选来源）。
     */
    private fun cfTrue(): CValuesRef<*>? = kCFBooleanTrue

    private companion object {
        const val KEY_CLASS = "class"
        const val CLASS_GENERIC_PASSWORD = "genp"
        const val KEY_ATTR_SERVICE = "svce"
        const val KEY_ATTR_ACCOUNT = "acct"
        const val KEY_RETURN_DATA = "r_Data"
        const val KEY_VALUE_DATA = "v_Data"
        const val KEY_ACCESSIBLE = "pdmn"
        const val ACCESSIBLE_THIS_DEVICE_ONLY = "AfterFirstUnlock"
    }
}

/** 钥匙串操作失败（写路径不可恢复）。 */
class KeychainException(
    message: String,
) : Exception(message)
