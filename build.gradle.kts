// 根构建文件只声明插件版本（经 version catalog），不做任何配置；
// 实际配置在各模块（packages/kmp-sdk、apps/android、apps/ios 构建脚本）。
plugins {
    alias(libs.plugins.kotlin.multiplatform) apply false
    alias(libs.plugins.kotlin.serialization) apply false
    alias(libs.plugins.android.library) apply false // AGP 9 内置 KMP 支持（com.android.kotlin.multiplatform.library）
    alias(libs.plugins.sqldelight) apply false
    alias(libs.plugins.detekt) apply false
    alias(libs.plugins.ktlint) apply false
    alias(libs.plugins.kover) apply false
    alias(libs.plugins.dokka) apply false
}
