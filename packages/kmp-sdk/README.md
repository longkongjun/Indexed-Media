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

未初始化

## 后续验证

`just check-kmp-sdk`
