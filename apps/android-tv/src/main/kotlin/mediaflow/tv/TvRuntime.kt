package mediaflow.tv

import android.app.Application
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.util.LruCache
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import mediaflow.client.core.SdkResult
import mediaflow.client.core.auth.KeystoreDeviceAuthStore
import mediaflow.client.core.net.MediaFlowApiFactory
import mediaflow.client.core.net.SessionAuthCache
import java.net.URI
import java.security.MessageDigest

/** Android持有Keystore、图片解码和进程生命周期；平台细节不进入共享SDK或Design库。 */
internal class TvViewModel(
    application: Application,
) : AndroidViewModel(application) {
    private val preferences = application.getSharedPreferences("mediaflow-tv", Context.MODE_PRIVATE)
    val model =
        TvModel(viewModelScope, sessionFactory = { server ->
            val identity = MessageDigest.getInstance("SHA-256").digest(server.toByteArray()).joinToString("") { "%02x".format(it) }
            val store = KeystoreDeviceAuthStore(application, application.filesDir.resolve("device-$identity.enc"))
            val credentials = SessionAuthCache()
            val factory = MediaFlowApiFactory(server, credentials)
            TvSession(server, store, credentials, factory.httpClient(), MediaFlowApiFactory.defaultJson())
        }, rememberServer = { server ->
            preferences
                .edit()
                .putString("server", server)
                .putBoolean("allow-http", server.startsWith("http://"))
                .apply()
        }, initialTheme = TvTheme.fromPreference(preferences.getString("theme", null)), rememberTheme = {
            preferences.edit().putString("theme", it.preference).apply()
        })

    init {
        viewModelScope.launch {
            val saved =
                withContext(Dispatchers.IO) {
                    preferences.getString("server", null) to preferences.getBoolean("allow-http", false)
                }
            saved.first?.takeIf { it.isNotBlank() }?.let { model.connect(it, saved.second, restore = true) }
        }
    }

    override fun onCleared() {
        model.close()
    }
}

/** 身份内有界内存缓存；调用者退出会话时清除，不落盘媒体或凭据。 */
internal class TvArtworkCache {
    private val images =
        object : LruCache<String, Bitmap>(MAX_CACHE_BYTES) {
            override fun sizeOf(
                key: String,
                value: Bitmap,
            ): Int = value.allocationByteCount
        }

    suspend fun load(
        model: TvModel,
        id: String,
        epoch: Long,
    ): Bitmap? {
        val key = "$epoch/$id"
        images.get(key)?.let { return it }
        val result = model.artwork(id)
        if (result !is SdkResult.Success) return null
        return withContext(Dispatchers.Default) {
            val bytes = result.value.bytes
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
            if (bounds.outWidth !in 1..MAX_DIMENSION ||
                bounds.outHeight !in 1..MAX_DIMENSION ||
                bounds.outWidth.toLong() * bounds.outHeight > MAX_PIXELS
            ) {
                return@withContext null
            }
            var sample = 1
            while (bounds.outWidth / sample > MAX_DECODED_DIMENSION || bounds.outHeight / sample > MAX_DECODED_DIMENSION) sample *= 2
            val options = BitmapFactory.Options().apply { inSampleSize = sample }
            val decoded = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options) ?: return@withContext null
            images.put(key, decoded)
            decoded
        }
    }

    fun clear() = images.evictAll()

    private companion object {
        const val MAX_CACHE_BYTES = 16 * 1024 * 1024
        const val MAX_DIMENSION = 4096
        const val MAX_PIXELS = 8_000_000
        const val MAX_DECODED_DIMENSION = 1024
    }
}

/** 仅委托系统打开HTTP(S)外部服务；缺少处理应用时由页面呈现恢复提示。 */
internal fun openTvPlayback(
    context: Context,
    url: String,
) {
    val uri = URI(url)
    require(uri.scheme?.lowercase() in setOf("http", "https") && uri.host != null && uri.userInfo == null) { "外部播放地址无效" }
    context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
}
