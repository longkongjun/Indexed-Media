# Web

## 目标

提供 MediaFlow 的 Vue Web/PWA 正式 UI。

## 职责

Vue Web/PWA 正式 UI。

## 允许依赖

`packages/ui-vue`、`packages/api-client-ts`、`packages/ui-contract`、`packages/test-fixtures`、`contracts`、`@indexed-design/ui-vue`（vendored tarball，见 `vendor/indexed-design/README.md`；接入、主题衔接与组件映射见 [docs/development/indexed-design-adoption.md](../../docs/development/indexed-design-adoption.md)）。

## 非目标

Core 业务逻辑、原生移动端或 TV UI、播放以及 `labs` 下的实验。

## 状态

已初始化：M2 Vue Web 身份、收件目录、显式 `/scan-tasks` 扫描链路、SSE 恢复和原始文件流程已有组件、可访问性、桌面/移动浏览器及单容器同源运行证据；M3 增加 `/tasks` ProcessingTask 中心、ReviewCase 人工决定、正式 `/media` Catalog，以及 `/organization/targets` 目标管理和任务详情内的计划/journal/result 恢复视图。M4 Change 1 增加 `/connections/downloaders` 连接管理与 `/downloads` 手动下载/监控页面；M4 Change 2 增加 `/automation/sources` 来源、事件与本地识别增强管理。409 保留安全草稿；organization 执行/回滚响应丢失先 GET 真值，只有用户显式重试才复用原幂等键。

设计系统已采纳（labs/design-system tokens v2.0）：全部样式经 `--mfd-*` 语义 token 变量链（无散落硬编码色）；双主题三态（跟随系统/浅色/暗色，`data-theme` + 防闪 + `mfd-theme` 记忆）与皮肤维度（evergreen/aurora/sepia，`data-skin`）在"账户与系统 → 外观"切换，切换过渡尊重 `prefers-reduced-motion`。状态徽章/词表走共享字典（`src/components/dictionaries.ts`，词表以 `packages/ui-contract` 文案索引为权威）；AsyncState 对齐六态容器规格（stale 降对比 + inert，conflict/partial/success 页面态由审批链承载）。审批链 UI 就位：任务详情页 approval-banner（needs_approval → `plan.authorization=paused` 驱动）→ plan-preview 全影响视图（批准 = Web 人工授权唯一产生方，二次确认绑定 `plan.version`；拒绝映射为取消任务）→ task-timeline 阶段留痕；AiSessionPanel 为审批深链入口（会话内无审批按钮）；导航含未读角标。审批语义对齐 SA-1-001 修复后的 core-ai-capability-protocol（capability 通道只交付不授权）。

媒体页只读取正式 Catalog；目标表单只提交 root ID、相对路径、固定操作/NFO 策略和有界规则，不接受宿主路径、脚本或 NFO 正文。Artwork 仅使用本地不透明引用和缺图状态，不拼接第三方图片 URL；页面不展示 Jellyfin 媒体级字段，也不把“决定已接受”描述成“已整理”。下载器表单不回填密码，测试、保存和异常路径都会清空凭据；下载源不进入任务列表/详情或响应丢失恢复状态。

## 后续验证

`just check-web` 验证类型、全量组件、生产构建/体积与 M2/M3/M4/审批链/主题桌面移动 Playwright；`just check-ui-contract` 验证页面状态权威包；设计 token 与对比度基线由 `labs/design-system` 自带检查维护；M3 人工审核聚合使用 `just check-m3-review-admin`，安全整理聚合使用 `just check-m3-safe-organization`，M4 Change 2 聚合使用 `CI=true just check-m4-source-automation`。真实文件/mount、RSS、下载器和 Ollama 验收由 Core 的独立 live 入口负责，Web mock/E2E 不替代它们。M2 单容器与目标 NAS 部署证据由对应运维门禁独立维护。

Web公开的build/typecheck/test/lint命令会先构建`@mediaflow/ui-vue`，不依赖工作区已有dist；容器Web阶段同时复制该包的manifest与源码。

Indexed-Design 接入（两批）：整树挂 `IndexedThemeProvider`（`app/designTheme.ts` 桥衔接宿主三态，皮肤维度暂不映射）；媒体列表/详情用 `Button`/`RequestState`；`AsyncState` 页面状态容器呈现层委托 `RequestState`（契约与 ui-contract 权威不变）；登录字段用 `Input`、收件预检确认用 `Modal`——视图层不再导入 `@mediaflow/ui-vue`（`MediaArtwork` 的 `MfImage` 按映射文档保留）。双真源过渡与映射见接入文档。该迁移不代表媒体页高保真、G1 或全产品验收完成。
