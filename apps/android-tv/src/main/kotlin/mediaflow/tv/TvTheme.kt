package mediaflow.tv

/** 设备本地偏好；显式模式独立于系统，System随当前平台配置解析。 */
internal enum class TvTheme(
    val preference: String,
) {
    System("system"),
    Light("light"),
    Dark("dark"),
    ;

    fun isDark(systemDark: Boolean): Boolean =
        when (this) {
            System -> systemDark
            Light -> false
            Dark -> true
        }

    companion object {
        fun fromPreference(value: String?): TvTheme = entries.firstOrNull { it.preference == value } ?: System
    }
}
