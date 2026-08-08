# Web

## 目标

提供 MediaFlow 的 Vue Web/PWA 正式 UI。

## 职责

Vue Web/PWA 正式 UI。

## 允许依赖

`packages/api-client-ts`、`packages/ui-contract`、`packages/test-fixtures`、`contracts`。

## 非目标

Core 业务逻辑、原生移动端或 TV UI、播放以及 `labs` 下的实验。

## 状态

已初始化：M2 Vue Web 身份、收件目录、显式 `/scan-tasks` 扫描链路、SSE 恢复和原始文件流程已有组件、可访问性、桌面/移动浏览器及单容器同源运行证据；M3 增加 `/tasks` ProcessingTask 中心、ReviewCase 人工决定、正式 `/media` Catalog，以及 `/organization/targets` 目标管理和任务详情内的计划/journal/result 恢复视图。M4 Change 1 增加 `/connections/downloaders` 连接管理与 `/downloads` 手动下载/监控页面；M4 Change 2 增加 `/automation/sources` 来源、事件与本地识别增强管理。409 保留安全草稿；organization 执行/回滚响应丢失先 GET 真值，只有用户显式重试才复用原幂等键。

媒体页只读取正式 Catalog；目标表单只提交 root ID、相对路径、固定操作/NFO 策略和有界规则，不接受宿主路径、脚本或 NFO 正文。Artwork 仅使用本地不透明引用和缺图状态，不拼接第三方图片 URL；页面不展示 Jellyfin 媒体级字段，也不把“决定已接受”描述成“已整理”。下载器表单不回填密码，测试、保存和异常路径都会清空凭据；下载源不进入任务列表/详情或响应丢失恢复状态。

## 后续验证

`just check-web` 验证类型、全量组件、生产构建/体积与 M2/M3/M4 桌面移动 Playwright；M3 人工审核聚合使用 `just check-m3-review-admin`，安全整理聚合使用 `just check-m3-safe-organization`，M4 Change 2 聚合使用 `CI=true just check-m4-source-automation`。真实文件/mount、RSS、下载器和 Ollama 验收由 Core 的独立 live 入口负责，Web mock/E2E 不替代它们。M2 单容器与目标 NAS 部署证据由对应运维门禁独立维护。
