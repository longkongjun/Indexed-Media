plugins {
    alias(libs.plugins.ktlint)
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "mediaflow.tv"
    compileSdk = 37
    defaultConfig {
        applicationId = "mediaflow.client.tv"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures { compose = true }
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.ui:ui")
    implementation(libs.androidx.tv.material)
    implementation(project(":packages:kmp-sdk"))
    implementation("dev.indexed.design:compose-tv:0.15.0")
    testImplementation("org.jetbrains.kotlin:kotlin-test-junit:2.4.21")
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.ktor.client.mock)
    androidTestImplementation(platform(libs.compose.bom))
    androidTestImplementation("org.jetbrains.kotlin:kotlin-test-junit:2.4.21")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation(libs.androidx.test.runner)
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
