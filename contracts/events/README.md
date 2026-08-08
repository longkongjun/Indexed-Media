# 事件契约

## 目标

为流式和 Webhook 消费者定义版本化事件接口。

## 职责

版本化 SSE 与 Webhook 事件 schema。

## 非目标

事件投递实现、broker 配置和客户端 UI。

## 状态

已初始化：`v1/task-event.schema.json` 定义兼容的 schema-v1 SSE 信封，包括 M2 scan 事件与 M3 的 `inbox.discovery-health-changed`、`processing-task.state-changed`、`processing-task.identification-decided`、`integration.health-changed`。所有事件来自事务 outbox；未知类型由客户端忽略，`Last-Event-ID` 与 `stream.gap` 语义不变。

## 后续验证

`just check-events`
