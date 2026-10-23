# KMP SDK

## 目标

为受支持客户端提供共享的 Kotlin Multiplatform 业务与数据逻辑。

## 职责

KMP SDK 公共 API：Ktor client、serialization、SQLDelight、repository、use case、错误映射和任务状态。

## 允许依赖

`contracts`；绝不能依赖 `apps` 或 `labs`。

## 非目标

正式 UI、应用生命周期代码、平台专属页面以及对任何应用或实验项目的依赖。

## 状态

脚手架已初始化（M5 第一批，分支 `feature/m5-client`）：

- **分层**：`mediaflow.api.*` 为构建期由 `contracts/openapi/mediaflow.v1.yaml` 生成的契约客户端（openapi-generator `kotlin`/`library=multiplatform`，经 JavaExec 驱动 7.26 fat jar；不手改、不进 lint）；`mediaflow.client.*` 为手写 SDK 核心。契约漂移由编译 + fixtures 回放测试（C1）拦截。
- **目标**：android（AGP 9 内置 KMP 插件）、jvm、iosArm64、iosSimulatorArm64。
- **门禁**：ktlint + detekt（`detekt.yml`）+ multiplatform test + Kover（排除生成代码）+ dokka；入口见根 `justfile` 的 `client-*` 目标与 `.github/workflows/client-ci.yml`。

## 后续验证

`just client-check-kmp-sdk`（macOS/Xcode：lint + 全目标测试 + 覆盖率 + 声明 Native 目标编译）或 `just client-check-kmp-jvm`（JVM 快速子集，不代签 Native）；`just client-generate` 单独重生成契约客户端；`just client-docs-kmp-sdk` 生成 API 文档。
