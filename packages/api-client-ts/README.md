# TypeScript API Client

## 目标

提供兼容客户端使用的 TypeScript API client。

## 职责

TypeScript client 公共 API：生成的 OpenAPI TypeScript client 与薄 transport wrapper。

## 允许依赖

`contracts`；绝不能依赖 `apps` 或 `labs`。

## 非目标

应用专属 UI、生命周期代码、手写服务端业务逻辑以及对任何应用或实验项目的依赖。

## 状态

已初始化：从 `contracts/openapi/mediaflow.v1.yaml` 生成 M2/M3/M4 DTO，并提供原生 fetch 薄 transport、TMDB/发现策略、ProcessingTask/识别、ReviewCase/正式 Catalog、organization target/preflight/任务命令、下载器连接/任务、自动来源/事件和本地识别增强方法。客户端只透传调用方持有的资源版本与幂等键，不在 transport 内猜测写意图或恢复策略。SSE helper 严格解析已知事件、拒绝非法结构并保持事件 ID 单调推进。公开类型不含密码、下载源、Feed URL、Webhook secret/signature/payload、模型输入输出、宿主/远端路径、NFO 正文或协议原始响应。

## 后续验证

`just check-api-client-ts` 验证类型、单测与生成漂移；跨 Core/Web 的 M3 人工审核与安全整理分别使用 `just check-m3-review-admin`、`just check-m3-safe-organization`，M4 Change 1 使用 `just check-m4-downloader-management`，M4 Change 2 使用 `CI=true just check-m4-source-automation`。
