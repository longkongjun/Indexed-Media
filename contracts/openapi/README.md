# OpenAPI 契约

## 目标

定义版本化 REST API schema 和生成客户端的源契约。

## 职责

版本化 REST API schema 和生成客户端的源契约。

## 非目标

服务端实现、生成客户端产物和客户端 UI。

## 状态

已初始化：`mediaflow.v1.yaml` 是 M2/M3/M4 REST API 的唯一来源。M3 公开配置/策略、任务/识别、ReviewCase 人工决定、正式 Catalog 只读查询，以及版本化 organization target、无副作用 preflight、计划/journal/result 详情和幂等重算/一次执行/安全回滚。契约只传 root ID 与相对路径，不公开宿主路径、NFO 正文或通用文件 mutation。

## 后续验证

`just check-openapi`；M3 安全整理跨层聚合使用 `just check-m3-safe-organization`。
