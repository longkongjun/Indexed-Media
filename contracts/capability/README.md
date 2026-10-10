# 能力契约（media-capability.v1）

## 目标

为 AI 控制面（DSH）与 MediaFlow Core 之间的能力调用、结果、事件与结构化错误定义版本化协议。

## 职责

`v1/` 四类消息信封 + 事件分页信封的 JSON Schema；能力清单枚举与副作用等级枚举；配套 golden 示例由 `contracts/examples/v1/capability/` 提供，双端（Rust / TypeScript）契约测试回放。

## 协议要点

- `protocol_version` 固定为 `media-capability.v1`，不随模型版本隐式变化。
- 调用必须携带 `request_id`、`execution.mode`、幂等键与 `deadline_ms`；`expected_revision` 防止基于过期事实执行。
- Core 不信任调用方校验：capability、输入 schema、路径与凭据在 Core 侧再次校验；调用输入不可包含绝对路径、shell、SQL 或凭据。
- 结果 `status` 与 `next_actions` 是稳定枚举；AI 只能依据结构化状态选择下一步动作。
- 事件由 Core 排序持久化（`sequence` 单调），断线按 cursor 补发；与 Web SSE（`events/v1`）相互独立。
- 错误信封提供机器可判断的 `category`、`retryable`、`required_action` 与脱敏 `safe_details`。

## KMP 可消费性约束（M5 预置）

字段命名 camelCase、可空性显式（`["T", "null"]`）、判别使用常量字符串字段而非 `oneOf` 载载判别；Kotlin 侧可由 `kotlinx.serialization` 直接映射，DTO 一律由本契约生成或回放验证，不手写漂移。

## 非目标

能力执行实现（网关与 registry 见 Core `capability` 模块）、传输层选择（HTTP/SSE，随 A1 落地）、Web 管理端 OpenAPI（见 `openapi/mediaflow.v1.yaml`）。

## 状态

已初始化：v1 信封 schema 冻结（调用 / 结果 / 错误 / 事件 / 事件页），能力名与枚举按 AI-first 目标架构 §4.2–§4.3 定义；A1 起网关按本契约实现，能力专属 `input`/`result` 子 schema 随各能力实现逐个冻结并先行提交。

## 后续验证

`just check-capability`（test-fixtures 回放 + Core 契约测试）。
