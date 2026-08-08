# Core

## 目标

提供 MediaFlow 本地优先的服务端产品能力。

## 职责

REST/OpenAPI、SSE、持久任务、文件操作、连接器、身份和管理。

## 允许依赖

初始化时只依赖 `contracts`；运行时 crate 保持在本模块内部。

## 非目标

播放、转码、站点聚合、必需的云端控制面和客户端 UI。

## 状态

已初始化：M2 Core 的启动安全门禁、单管理员会话、能力根/收件目录、持久扫描、事务 outbox 与可恢复 SSE 已实现；M3 Change 1 增加连续发现、不可变 file revision、单文件 ProcessingTask、只读 Kodi NFO、加密 TMDB 配置、有界 provider/cache 和可解释识别决定。M3 Change 2 已实现不可变人工决定、重识别与重启协调、任务中心查询、ReviewCase 候选/API，以及只接受已核对 `LocalResult` 的规范化 Catalog。M3 安全整理纵向切片现已接通版本化目标/规则、不可变计划、capability-based copy/move/hardlink、journal 恢复、缺失 NFO 生成、`LocalResult`、Catalog 和认证命令。M4 Change 1 增加 qBittorrent/Transmission 连接、手动下载、持久租约监控、恢复与最小下载事件；M4 Change 2 增加 RSS/Atom、签名 Webhook、下载完成映射、持久 automation event 与默认关闭的本地 Ollama 识别增强。

识别和人工决定阶段仍只读取能力根内媒体/NFO；只有 organization coordinator 能在明确的 `read-write` deployment root 内执行持久计划。文件操作逐组件 no-follow、no-clobber，并先提交 journal；已有 NFO 永不覆盖，通用脚本/正则、任意路径浏览、跨文件批事务和未知数据删除均不支持。安全回滚只补偿仍与 verified journal 一致的本次产物，外部变化转人工处理。实例密钥固定为 `/config/instance.key`，首次创建为 32 个随机字节和 `0600`，TMDB Token、下载器凭据与下载源只以 XChaCha20-Poly1305 密文进入 SQLite。下载运行时只监控 MediaFlow 自己创建的任务，不提供暂停、删除下载或删除数据能力。

## 后续验证

`just check-core` 验证启动与 SQLite 基线；完整 M2 Core 回归使用 `just test-core-events`。M3 人工审核聚合门禁为 `just check-m3-review-admin`，安全整理自动聚合为 `just check-m3-safe-organization`。`just test-m3-organization-live` 只在显式提供两个互不重叠的隔离根和确认 sentinel 时运行；它只清理脚本创建的 UUID 子目录，未提供真实 mount/NAS 时明确 `SKIPPED` 且不算真实验收通过。变量和限制见脚本；本地开发能力根示例为 [`docs/development/deployment-roots.organization.example.json`](../../docs/development/deployment-roots.organization.example.json)。M4 Change 1 自动门禁为 `just check-m4-downloader-management`，真实协议验收使用独立 `just test-m4-downloaders-live`。M4 Change 2 自动聚合为 `CI=true just check-m4-source-automation`；`just test-m4-source-automation-live` 仅在脚本列出的真实 RSS、两个下载器、本地 Ollama、隔离 root 和确认 sentinel 全部提供时运行，缺项明确 `SKIPPED/DEFERRED`，且不会删除创建的下载任务或任何非脚本所有的目录。
