package mediaflow.client.core.auth

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import java.io.File
import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Android Keystore 加密的凭据存储：AES-256-GCM 密钥由 AndroidKeyStore 持有
 * （不可导出），密文落 App 私有目录文件。密钥随设备生成，清除 App 数据即失效。
 *
 * 失读（密钥被 Invalidate/文件损坏）按"无凭据"处理——用户重新配对即可，
 * 不抛异常打断启动路径。
 */
class KeystoreDeviceAuthStore(
    context: Context,
    private val file: File = context.filesDir.resolve("device-credentials.enc"),
) : DeviceAuthStore {
    private val json = Json { ignoreUnknownKeys = true }
    private val keyAlias = "${context.packageName}.device-credentials"

    override suspend fun read(): StoredCredentials? =
        withContext(Dispatchers.IO) {
            val bytes = file.takeIf { it.exists() }?.readBytes() ?: return@withContext null
            runCatching { decrypt(bytes.decodeToString()) }
                .map { json.decodeFromString(StoredCredentials.serializer(), it) }
                .getOrNull()
        }

    override suspend fun save(credentials: StoredCredentials) {
        withContext(Dispatchers.IO) {
            file.parentFile?.mkdirs()
            file.writeText(encrypt(json.encodeToString(StoredCredentials.serializer(), credentials)))
        }
    }

    override suspend fun clear() {
        withContext(Dispatchers.IO) {
            check(!file.exists() || file.delete()) { "device credential file could not be removed" }
        }
    }

    private fun encrypt(plaintext: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val ciphertext = cipher.doFinal(plaintext.toByteArray(Charsets.UTF_8))
        // IV(12B) 与密文拼接后 base64；解密侧按前缀还原 IV。
        val combined = cipher.iv + ciphertext
        return Base64.getEncoder().encodeToString(combined)
    }

    private fun decrypt(stored: String): String {
        val combined = Base64.getDecoder().decode(stored)
        require(combined.size > GCM_IV_BYTES) { "ciphertext too short" }
        val iv = combined.copyOfRange(0, GCM_IV_BYTES)
        val ciphertext = combined.copyOfRange(GCM_IV_BYTES, combined.size)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(GCM_TAG_BITS, iv))
        return String(cipher.doFinal(ciphertext), Charsets.UTF_8)
    }

    private fun key(): SecretKey {
        val keyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (keyStore.getKey(keyAlias, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        generator
            .init(
                KeyGenParameterSpec
                    .Builder(keyAlias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(AES_KEY_BITS)
                    .build(),
            )
        return generator.generateKey()
    }

    private companion object {
        const val ANDROID_KEYSTORE = "AndroidKeyStore"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val GCM_IV_BYTES = 12
        const val GCM_TAG_BITS = 128
        const val AES_KEY_BITS = 256
    }
}
