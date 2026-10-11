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
import platform.CoreFoundation.CFPropertyListCreateWithData
import platform.CoreFoundation.CFRelease
import platform.CoreFoundation.CFStringCreateWithCString
import platform.CoreFoundation.CFStringRef
import platform.CoreFoundation.CFTypeRefVar
import platform.Foundation.NSData
import platform.Security.SecItemAdd
import platform.Security.SecItemCopyMatching
import platform.Security.SecItemDelete
import platform.Security.SecItemUpdate
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
        // 先查后写：存在则 Update，否则 Add（Keychain 无 upsert 原语）。
        if (query() != null) {
            update(payload)
        } else {
            add(payload)
        }
    }

    override suspend fun clear() {
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
            val accessibleKey = cfstr(KEY_ACCESSIBLE)
            val accessibleValue = cfstr(ACCESSIBLE_THIS_DEVICE_ONLY)
            owned.addAll(listOf(valueKey, accessibleKey, accessibleValue))
            CFDictionaryAddValue(
                dictionary,
                valueKey,
                CFDataCreate(null, payload.toUByteArray().toCValues(), payload.size.toLong()),
            )
            CFDictionaryAddValue(dictionary, accessibleKey, accessibleValue)
            val status = SecItemAdd(dictionary, null)
            CFRelease(dictionary)
            owned.forEach { CFRelease(it) }
            if (status != errSecSuccess) throw KeychainException("keychain add failed: $status")
        }

    private fun update(payload: ByteArray): Unit =
        memScoped {
            val owned = mutableListOf<CFStringRef>()
            val search = CFDictionaryCreateMutable(null, 0, null, null)!!
            fillBase(search, owned)
            val updates = CFDictionaryCreateMutable(null, 0, null, null)!!
            val valueKey = cfstr(KEY_VALUE_DATA)
            owned.add(valueKey)
            CFDictionaryAddValue(
                updates,
                valueKey,
                CFDataCreate(null, payload.toUByteArray().toCValues(), payload.size.toLong()),
            )
            val status = SecItemUpdate(search, updates)
            CFRelease(updates)
            CFRelease(search)
            owned.forEach { CFRelease(it) }
            if (status != errSecSuccess) throw KeychainException("keychain update failed: $status")
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
     * 构造真正的 CFBoolean：interop 对 `kCFBooleanTrue` 暴露的指针在运行时不是
     * 合法对象（Security DER 序列化时崩溃），经 plist 解析 `<true/>` 拿到 CFBoolean
     * 单例是确定性的等价物。返回对象由调用方保证生命周期（查询即用即弃）。
     */
    private fun cfTrue(): CValuesRef<*>? =
        memScoped {
            val xml = "<true/>".encodeToByteArray()
            val data = CFDataCreate(null, xml.toUByteArray().toCValues(), xml.size.toLong()) ?: return@memScoped null
            val parsed = alloc<CFTypeRefVar>()
            val created = CFPropertyListCreateWithData(null, data, 0u, null, null)
            CFRelease(data)
            created ?: parsed.value
        }

    private companion object {
        const val KEY_CLASS = "class"
        const val CLASS_GENERIC_PASSWORD = "genp"
        const val KEY_ATTR_SERVICE = "svce"
        const val KEY_ATTR_ACCOUNT = "acct"
        const val KEY_RETURN_DATA = "r_Data"
        const val KEY_VALUE_DATA = "v_Data"
        const val KEY_ACCESSIBLE = "pdmn"
        const val ACCESSIBLE_THIS_DEVICE_ONLY = "AfterFirstUnlockThisDeviceOnly"
    }
}

/** 钥匙串操作失败（写路径不可恢复）。 */
class KeychainException(
    message: String,
) : Exception(message)
