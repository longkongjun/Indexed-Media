package mediaflow.client

import kotlin.test.Test
import kotlin.test.assertEquals

class SdkVersionTest {
    @Test
    fun versionIsSemverShaped() {
        // 冒烟：验证多平台测试链路就位；版本语义检查在 C1 引入契约测试后收紧。
        assertEquals(3, SdkVersion.VERSION.split('.').size)
    }
}
