// MediaFlow 客户端与原生基础库的 Gradle 工程入口。
// 仓库其余工程（Rust/pnpm）不受本文件影响；本工程只包含客户端模块。
pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode = RepositoriesMode.FAIL_ON_PROJECT_REPOS
    repositories {
        google()
        mavenCentral()
        exclusiveContent {
            forRepository {
                maven {
                    name = "indexedDesignVendored"
                    url = uri("vendor/indexed-design/maven")
                    metadataSources { gradleMetadata() }
                }
            }
            filter {
                includeModule("dev.indexed.design", "compose-desktop")
                includeModule("dev.indexed.design", "compose-tv")
            }
        }
    }
}

rootProject.name = "mediaflow-clients"

include(":packages:kmp-sdk")
include(":apps:android")
include(":tools:client-fixture-server")

include(":packages:ui-android")
include(":packages:ui-android-tv")
include(":tools:ui-android-consumer")
include(":tools:ui-tv-consumer")

include(":apps:desktop")
include(":apps:android-tv")
