# 能力契约（media-capability.v1）

## 目标

为 AI 控制面（DSH）与 MediaFlow Core 之间的能力调用、结果、事件与结构化错误定义版本化协议。

## 职责

`v1/` 四类消息信封 + 事件分页信封的 JSON Schema；能力清单枚举与副作用等级枚举；配套 golden 示例由 `contracts/examples/v1/capability/` 提供，双端（Rust / TypeScript）契约测试回放。

## 协议要点

- `protocol_version` 固定为 `media-capability.v1`，不随模型版本隐式变化。
- 调用必须携带 `request_id`、`execution.mode`、幂等键与 `deadline_ms`；`expected_revision` 防止基于过期事实执行。
- Core 不信任调用方校验：capability、输入 schema、路径与凭据在 Core 侧再次校验；调用输入不可包含绝对路径、shell、SQL 或凭据。
- 结果 `request_id`：可解析且带有效 UUID 的调用在成功和拒绝时均返回原 ID（认证失败也不执行或持久化调用）；无法解析或无有效调用 ID 时返回 Core 新生成的 UUID，仅用于本次响应关联，不代表调用已被接受。事件查询错误没有调用信封，同样使用服务端关联 ID。
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

### 重放身份与未知结果

当前网关以反序列化后的完整调用信封规范化指纹绑定 `request_id`，包含声明 actor、session、task、输入及执行约束；对象键顺序与 JSON 空白不影响等价性。认证主体仍由服务端 capability 令牌通道确定，声明 actor 不增加授权权限。同 ID 内容变化、同能力同幂等键被另一请求占用均稳定拒绝。

预检通过后的新调用先持久取得唯一预约，完成结果、事件与完成标记在同一 SQLite 短事务提交。进程退出或持久化失败留下的 pending 预约不会自动过期、抢占或重新执行；调用者应按原 task/operation 查询领域事实，不能换 request/idempotency key 盲目重发文件操作。领域 journal、人工审批和业务回执仍是副作用恢复依据。

升级前缺少原信封指纹的历史调用保持审计数据；重用其请求 ID 或能力幂等键将保守拒绝，不猜测内容等价。不应降级到不理解预约表的旧版本后重放未知操作，需先核对任务与 journal。以上不改变 `media-capability.v1` 字段或 AI 只交付、不自行批准的边界。
