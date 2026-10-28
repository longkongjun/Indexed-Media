import org.jetbrains.compose.desktop.application.dsl.TargetFormat
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.kotlin.jvm)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.compose.multiplatform)
    alias(libs.plugins.ktlint)
}

kotlin { compilerOptions { jvmTarget.set(JvmTarget.JVM_21) } }
java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
dependencies {
    implementation(project(":packages:kmp-sdk"))
    implementation(libs.sqldelight.driver.sqlite)
    implementation("dev.indexed.design:compose-desktop:0.15.0")
    implementation(compose.desktop.currentOs)
    implementation(compose.material3)
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-swing:${libs.versions.kotlinx.coroutines.get()}")
    implementation("net.java.dev.jna:jna:${libs.versions.jna.get()}")
    testImplementation(compose.desktop.uiTestJUnit4)
    testImplementation(kotlin("test-junit"))
    testImplementation(libs.kotlinx.coroutines.test)
}
compose.desktop {
    application {
        mainClass = "mediaflow.desktop.MainKt"
        nativeDistributions {
            targetFormats(TargetFormat.Dmg, TargetFormat.Msi)
            packageName = "MediaFlow"
            packageVersion = "0.1.0"
            macOS {
                packageVersion = "1.1.0"
                minimumSystemVersion = "13.0"
            }
            modules("java.sql", "java.desktop", "java.naming", "jdk.unsupported", "jdk.crypto.ec")
        }
    }
}

// The ordinary JVM suite stays usable without a Rust build or a live backend.
tasks.test { exclude("**/DesktopRealCoreTest*") }
tasks.register<Test>("realCoreTest") {
    group = "verification"
    description = "Runs the desktop SDK/model against a newly isolated real Core process"
    testClassesDirs =
        sourceSets.test
            .get()
            .output.classesDirs
    classpath = sourceSets.test.get().runtimeClasspath
    filter { includeTestsMatching("mediaflow.desktop.DesktopRealCoreTest") }
    outputs.upToDateWhen { false }
    outputs.cacheIf { false }
}
