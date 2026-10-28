package mediaflow.desktop

import kotlinx.coroutines.runBlocking
import mediaflow.client.core.auth.StoredCredentials
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotEquals
import kotlin.test.assertNull

class PlatformTest {
    @Test fun serverIdentityRejectsAmbiguousAddresses() {
        assertEquals("https://nas.example:8443", normalizeServer(" https://NAS.example:8443/ "))
        for (url in listOf("file:///tmp/x", "https://name:secret@nas", "https://nas/a", "https://nas?x=1", "https://nas#x")) {
            assertFailsWith<IllegalArgumentException> { normalizeServer(url) }
        }
        assertNotEquals(cacheIdentity("https://a", "device"), cacheIdentity("https://b", "device"))
        assertNotEquals(cacheIdentity("https://a", "one"), cacheIdentity("https://a", "two"))
    }

    @Test fun credentialStoreRoundTripAndClearUseVaultOnly() =
        runBlocking {
            val vault = MemoryVault()
            val store = VaultAuthStore(vault)
            assertNull(store.read())
            val value = StoredCredentials.DeviceToken("synthetic-token", "synthetic-device", "Test")
            store.save(value)
            assertEquals(value, store.read())
            store.clear()
            assertNull(store.read())
        }

    @Test fun windowsCredentialLayoutMatchesNative64BitAbi() {
        if (com.sun.jna.Native.POINTER_SIZE != 8) return
        val value = Credential()
        assertEquals(80, value.size())
        assertEquals(24, value.offset("lastWrittenLow"))
        assertEquals(32, value.offset("blobSize"))
        assertEquals(40, value.offset("blob"))
        assertEquals(72, value.offset("userName"))
    }

    private class MemoryVault : CredentialVault {
        private var bytes: ByteArray? = null

        override fun read(): ByteArray? = bytes?.copyOf()

        override fun write(value: ByteArray) {
            bytes = value.copyOf()
        }

        override fun delete() {
            bytes = null
        }
    }
}
