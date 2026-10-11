plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization) // AGP 9 内置 Kotlin 工具链
}

android {
    namespace = "mediaflow.androidapp"
    compileSdk = 37

    defaultConfig {
        applicationId = "mediaflow.client.android"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false // 第一批 fixture 驱动切片；发布裁剪随第二批真机交付
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }
}

dependencies {
    val composeBom = platform(libs.compose.bom)
    implementation(composeBom)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    debugImplementation("androidx.compose.ui:ui-tooling")

    implementation(project(":packages:kmp-sdk"))

    testImplementation("org.jetbrains.kotlin:kotlin-test-junit:2.4.21")
    testImplementation(libs.kotlinx.coroutines.test)
}
