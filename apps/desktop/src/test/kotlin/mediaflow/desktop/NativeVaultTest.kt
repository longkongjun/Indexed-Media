package mediaflow.desktop

import org.junit.Assume.assumeTrue
import java.util.UUID
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertNull

class NativeVaultTest {
    @Test fun operatingSystemVaultSyntheticRoundTrip() {
        val os = System.getProperty("os.name")
        assumeTrue("Native credential test requires macOS or Windows", os.startsWith("Mac") || os.startsWith("Windows"))
        val target = "MediaFlow/test/${UUID.randomUUID()}"
        val vault = if (os.startsWith("Mac")) MacKeychain(target) else WindowsCredentials(target)
        try {
            assertNull(vault.read())
            vault.write("synthetic-first".toByteArray())
            assertContentEquals("synthetic-first".toByteArray(), vault.read())
            vault.write("synthetic-replacement".toByteArray())
            assertContentEquals("synthetic-replacement".toByteArray(), vault.read())
            vault.delete()
            assertNull(vault.read())
        } finally {
            vault.delete()
        }
    }
}
