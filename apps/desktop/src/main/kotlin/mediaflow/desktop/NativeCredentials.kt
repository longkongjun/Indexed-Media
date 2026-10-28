package mediaflow.desktop

import com.sun.jna.Memory
import com.sun.jna.Native
import com.sun.jna.NativeLibrary
import com.sun.jna.Pointer
import com.sun.jna.Structure
import com.sun.jna.WString
import com.sun.jna.ptr.IntByReference
import com.sun.jna.ptr.PointerByReference
import com.sun.jna.win32.StdCallLibrary

/** JNA calls only system credential APIs; no subprocess receives credential material. */
internal class MacKeychain(
    private val target: String,
) : CredentialVault {
    private val security = NativeLibrary.getInstance("Security")
    private val foundation = NativeLibrary.getInstance("CoreFoundation")
    private val service = target.toByteArray()
    private val account = "device".toByteArray()

    private fun find(
        length: IntByReference,
        data: PointerByReference,
        item: PointerByReference,
    ): Int =
        security
            .getFunction(
                "SecKeychainFindGenericPassword",
            ).invokeInt(arrayOf(null, service.size, service, account.size, account, length, data, item))

    private fun checkStatus(status: Int) {
        check(status == 0) { "Keychain 操作失败（$status）" }
    }

    private fun release(item: Pointer?) {
        if (item != null) foundation.getFunction("CFRelease").invokeVoid(arrayOf(item))
    }

    override fun read(): ByteArray? {
        val length = IntByReference()
        val data = PointerByReference()
        val item = PointerByReference()
        val status = find(length, data, item)
        if (status == -25300) return null
        checkStatus(status)
        return try {
            check(length.value in 1..2560) { "Keychain 凭据长度无效" }
            data.value.getByteArray(0, length.value)
        } finally {
            security.getFunction("SecKeychainItemFreeContent").invokeInt(arrayOf(null, data.value))
            release(item.value)
        }
    }

    override fun write(value: ByteArray) {
        require(value.isNotEmpty() && value.size <= 2560) { "凭据长度无效" }
        val length = IntByReference()
        val data = PointerByReference()
        val item = PointerByReference()
        val status = find(length, data, item)
        if (status == -25300) {
            checkStatus(
                security
                    .getFunction(
                        "SecKeychainAddGenericPassword",
                    ).invokeInt(arrayOf(null, service.size, service, account.size, account, value.size, value, null)),
            )
        } else {
            checkStatus(status)
            try {
                checkStatus(
                    security.getFunction("SecKeychainItemModifyAttributesAndData").invokeInt(arrayOf(item.value, null, value.size, value)),
                )
            } finally {
                security.getFunction("SecKeychainItemFreeContent").invokeInt(arrayOf(null, data.value))
                release(item.value)
            }
        }
    }

    override fun delete() {
        val length = IntByReference()
        val data = PointerByReference()
        val item = PointerByReference()
        val status = find(length, data, item)
        if (status == -25300) return
        checkStatus(status)
        try {
            checkStatus(security.getFunction("SecKeychainItemDelete").invokeInt(arrayOf(item.value)))
        } finally {
            security.getFunction("SecKeychainItemFreeContent").invokeInt(arrayOf(null, data.value))
            release(item.value)
        }
    }
}

// Native entry point names must match the Windows exported symbols.
@Suppress("ktlint:standard:function-naming")
internal interface CredentialApi : StdCallLibrary {
    fun CredReadW(
        target: WString,
        type: Int,
        flags: Int,
        output: PointerByReference,
    ): Boolean

    fun CredWriteW(
        credential: Credential,
        flags: Int,
    ): Boolean

    fun CredDeleteW(
        target: WString,
        type: Int,
        flags: Int,
    ): Boolean

    fun CredFree(pointer: Pointer)
}

@Structure.FieldOrder(
    "flags",
    "type",
    "targetName",
    "comment",
    "lastWrittenLow",
    "lastWrittenHigh",
    "blobSize",
    "blob",
    "persist",
    "attributeCount",
    "attributes",
    "targetAlias",
    "userName",
)
class Credential(
    pointer: Pointer? = null,
) : Structure(pointer) {
    @JvmField var flags = 0

    @JvmField var type = 1

    @JvmField var targetName: WString? = null

    @JvmField var comment: WString? = null

    @JvmField var lastWrittenLow = 0

    @JvmField var lastWrittenHigh = 0

    @JvmField var blobSize = 0

    @JvmField var blob: Pointer? = null

    @JvmField var persist = 2

    @JvmField var attributeCount = 0

    @JvmField var attributes: Pointer? = null

    @JvmField var targetAlias: WString? = null

    @JvmField var userName: WString? = null

    internal fun offset(name: String): Int = fieldOffset(name)
}

internal class WindowsCredentials(
    private val target: String,
) : CredentialVault {
    private val api = Native.load("Advapi32", CredentialApi::class.java)

    override fun read(): ByteArray? {
        val output = PointerByReference()
        if (!api.CredReadW(WString(target), 1, 0, output)) {
            val code = Native.getLastError()
            if (code == 1168) return null
            error("Credential Manager 读取失败（$code）")
        }
        return try {
            val credential = Credential(output.value).also { it.read() }
            check(credential.blobSize in 1..2560) { "凭据长度无效" }
            checkNotNull(credential.blob) { "凭据内容缺失" }.getByteArray(0, credential.blobSize)
        } finally {
            api.CredFree(output.value)
        }
    }

    override fun write(value: ByteArray) {
        require(value.isNotEmpty() && value.size <= 2560) { "凭据长度无效" }
        val memory = Memory(value.size.toLong())
        try {
            memory.write(0, value, 0, value.size)
            val credential =
                Credential().apply {
                    targetName = WString(target)
                    userName = WString("MediaFlow device")
                    blob = memory
                    blobSize = value.size
                    write()
                }
            check(api.CredWriteW(credential, 0)) { "Credential Manager 写入失败（${Native.getLastError()}）" }
        } finally {
            memory.clear()
            memory.close()
        }
    }

    override fun delete() {
        if (!api.CredDeleteW(WString(target), 1, 0)) {
            val code = Native.getLastError()
            check(code == 1168) { "Credential Manager 删除失败（$code）" }
        }
    }
}
