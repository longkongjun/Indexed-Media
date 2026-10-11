// MediaFlow 客户端（M5）gradle 工程入口：KMP SDK 与移动端应用。
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
    }
}

rootProject.name = "mediaflow-clients"

include(":packages:kmp-sdk")
include(":apps:android")
include(":tools:client-fixture-server")
