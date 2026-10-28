package mediaflow.desktop

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import mediaflow.client.core.media.Artwork
import org.junit.Test
import java.awt.image.BufferedImage
import java.io.ByteArrayOutputStream
import javax.imageio.ImageIO
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull

class DesktopPosterTest {
    @Test
    fun rejectsCorruptAndOversizedImagesAndScalesValidPoster() {
        assertNull(decodeDesktopPoster(byteArrayOf(1, 2, 3)))
        assertNull(decodeDesktopPoster(ByteArray(2 * 1024 * 1024 + 1)))
        assertNull(decodeDesktopPoster(png(4097, 1)))
        assertNull(decodeDesktopPoster(png(3000, 3000)))
        val image = assertNotNull(decodeDesktopPoster(png(1200, 1800)))
        assertEquals(682, image.width)
        assertEquals(1024, image.height)
    }

    @Test
    fun boundsCacheAndReusesImagesUntilEvicted() =
        runBlocking {
            val cache = DesktopPosterCache(64)
            var calls = 0
            val load: suspend (String) -> Artwork? = {
                calls++
                Artwork(png(4, 4), "image/png")
            }
            assertNotNull(cache.load("a", load))
            assertNotNull(cache.load("a", load))
            assertEquals(1, calls)
            assertNotNull(cache.load("b", load))
            assertNotNull(cache.load("a", load))
            assertEquals(3, calls)
            cache.close()
            assertNull(cache.load("a", load))
            assertEquals(3, calls)
        }

    @Test
    fun closingCacheRejectsLateResponse() =
        runBlocking {
            val cache = DesktopPosterCache()
            val started = CompletableDeferred<Unit>()
            val reply = CompletableDeferred<Artwork>()
            val pending =
                async {
                    cache.load("a") {
                        started.complete(Unit)
                        reply.await()
                    }
                }
            started.await()
            cache.close()
            reply.complete(Artwork(png(2, 3), "image/png"))
            assertNull(pending.await())
        }

    @Test
    fun onlyFourFetchesCanRunAtOnce() =
        runBlocking {
            val cache = DesktopPosterCache()
            val firstFour = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            var entered = 0
            val pending =
                (1..8).map { index ->
                    async {
                        cache.load(index.toString()) {
                            entered++
                            if (entered == 4) firstFour.complete(Unit)
                            release.await()
                            null
                        }
                    }
                }
            try {
                kotlinx.coroutines.withTimeout(10_000) { firstFour.await() }
                assertEquals(4, entered)
            } finally {
                release.complete(Unit)
            }
            pending.forEach { it.await() }
            assertEquals(8, entered)
            cache.close()
        }

    private fun png(
        width: Int,
        height: Int,
    ): ByteArray =
        ByteArrayOutputStream().use { output ->
            ImageIO.write(BufferedImage(width, height, BufferedImage.TYPE_INT_RGB), "png", output)
            output.toByteArray()
        }
}
