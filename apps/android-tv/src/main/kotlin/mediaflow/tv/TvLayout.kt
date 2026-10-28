package mediaflow.tv

import kotlin.math.ceil

/** Design的2:3海报、固定标题区和网格留白；向上取列数才不会超过高度上限。 */
internal fun tvPosterColumns(
    width: Float,
    height: Float,
    fontScale: Float,
): Int {
    val caption = 90f * fontScale + 8f
    val poster = minOf(156f, ((height - 16f - caption) * 2f / 3f).coerceAtLeast(1f))
    return ceil((width - 16f + 24f) / (poster + 24f)).toInt().coerceAtLeast(2)
}
