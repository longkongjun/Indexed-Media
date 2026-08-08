# 契约

## 目标

MediaFlow 采用契约优先方式：实现前由版本化契约定义 Core 与所有客户端之间的接口。

## 职责

版本化 REST、事件和共享 payload 样例契约。

## 非目标

业务逻辑、客户端实现、生成构建产物和部署配置。

## 状态

已初始化：M2 身份、收件目录、扫描与 SSE 契约保持兼容；M3 增加 TMDB/发现、ProcessingTask/识别、版本化 ReviewCase/人工决定、正式 Catalog，以及 organization target/preflight/计划/journal/result/重算/一次执行/回滚和两个最小变化事件；M4 Change 1 增加 qBittorrent/Transmission 连接、候选测试、手动下载、任务查询和 `download-task-changed` 最小事件；M4 Change 2 增加自动来源/事件、本地识别增强和三个最小变化事件。网络契约不返回凭据、Cookie、下载源、Feed URL、Webhook secret/signature/payload、模型输入输出、tracker passkey、远端/宿主绝对路径、NFO 原文、第三方原始响应或远程 Artwork URL。

## 后续验证

`just check-contracts` 验证源契约；`just check-m3-identification-contracts` 保留 M3 Change 1 兼容入口；M3 人工审核与安全整理分别使用 `just check-m3-review-admin`、`just check-m3-safe-organization`；M4 Change 1 使用 `just check-m4-downloader-management`，M4 Change 2 使用 `CI=true just check-m4-source-automation`。
