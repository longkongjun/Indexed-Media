# Test Fixtures

## 目标

为跨端验证提供稳定的共享数据。

## 职责

公共 fixture API：版本化跨端 API 样例和端到端场景数据。

## 允许依赖

`contracts`；绝不能依赖 `apps` 或 `labs`。

## 非目标

应用专属 UI、生命周期代码、正式行为以及对任何应用或实验项目的依赖。

## 状态

已初始化：M2/M3/M4 fixture 直接重用 `contracts/examples/v1/`，类型检查和测试通过 AJV 验证对应 REST/SSE schema。M3 覆盖 TMDB/发现、ProcessingTask/identification、ReviewCase/人工决定、正式 Catalog，以及 organization target/preflight/paused plan/verified journal/partial result/rollback 与最小事件；M4 覆盖下载器连接/测试、下载任务、自动来源/事件、本地识别增强和最小变化事件。反样例持续拒绝凭据、下载源、Feed URL、Webhook secret/signature/payload、模型输入输出、宿主/远端路径、NFO 正文、敏感字段及过大 payload。

## 后续验证

`just check-test-fixtures` 验证类型与 fixture 测试；M3 人工审核和安全整理聚合入口分别为 `just check-m3-review-admin`、`just check-m3-safe-organization`，M4 Change 1 聚合入口为 `just check-m4-downloader-management`，M4 Change 2 为 `CI=true just check-m4-source-automation`。
