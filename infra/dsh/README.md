# DSH 接入（media-capability.v1）

## 目标

为 AI 控制面（DSH，DeepSeek Harness）的部署者提供接入 MediaFlow Core 的唯一权威路径：端点、认证、协议版本锁定与降级语义。Core 侧不包含 DSH 客户端代码——调用方向恒为 DSH → Core，防腐层就是能力网关本身。

## 协议版本锁定

- 协议：`media-capability.v1`（契约唯一事实源：`contracts/capability/v1/` JSON Schema + `contracts/examples/v1/capability/` golden 示例）。
- 版本不随模型版本隐式变化；破坏性变更只在阶段边界开窗，届时以新版本号并行提供。
- DSH 侧必须按 schema 校验发送与接收，未登记能力、未知字段与错误版本都会被稳定拒绝（`rejected` + `validation`）。

## 端点与认证

| 端点 | 方法 | 说明 |
|---|---|---|
| `/api/capability/v1/calls` | POST | 能力调用信封（`CapabilityCall`） |
| `/api/capability/v1/events?cursor=&limit=` | GET | 事件按 `sequence` cursor 分页拉取（`CapabilityEventPage`） |

认证：部署时注入的 Bearer 令牌（compose 环境变量 `MEDIAFLOW_CAPABILITY_TOKEN`，至少 16 字符）。未配置令牌时网关整体稳定拒绝（`provider` 分类），不影响管理员路由与确定性任务。令牌不可轮换为管理员会话凭据。

## 调用约束（DSH 侧义务）

- 每次调用携带唯一 `request_id`（重放返回缓存结果）与幂等键（同键不同请求稳定拒绝）。
- 写能力（`reversible-write` 模式）必须以 `expected_revision`（`plan.version:<n>` / `result.version:<n>`）精确绑定事实版本。
- 输入不得包含宿主绝对路径、shell 控制序列、SQL 语句形态或凭据字段——命中即 `validation` 拒绝。
- 一切失败都是 HTTP 200 的结构化信封（`status` + `error.category/retryable/required_action`）；按 `next_actions` 编排，不得把自然语言当执行成功。

## 事件消费

- 已持久化结果的能力调用产生 `capability.completed` / `capability.failed`；等待审批的计划产生 `task.waiting_approval`。
- 断线恢复：携带上一页 `next_cursor`（`capability-events:<sequence>`）继续拉取；序号严格递增、无缺口、无重复。
- 预检拒绝（认证/信封/白名单/revision）不留事件——DSH 以同步响应为准确认这些调用的结局。

## 降级语义（双方契约）

| 故障 | 行为 |
|---|---|
| DSH 停机/不可达 | Core 确定性任务、已入库资源与 AI 通道事件留痕完全不受影响 |
| DSH 超时（`deadline_ms` 超界） | 调用拒绝；无隐式后台执行 |
| DSH 输出非法信封 | 稳定 `rejected`，Core 状态零变化（任务版本与事件流不因恶意流量改变） |
| 重复提交 | 同 `request_id` 幂等重放；同幂等键不同请求稳定冲突，通道不卡死 |
| Core 重启 | 事件按 cursor 完整补发；写能力经 journal 恢复收敛 |

## 已登记能力（截至 A4）

| 能力 | 模式 | 说明 |
|---|---|---|
| `media.task-status` | read-only | 处理任务状态查询 |
| `media.match-candidates` | read-only | 待确认记录候选查询 |
| `media.create-plan` | plan-only | 计划预览；`dry_run=true` 重算并持久化新版本 |
| `media.request-approval` | approval-required | 如实反馈计划授权状态（`paused` → `needs_approval` + `task.waiting_approval` 事件）；审批动作由管理员在 Web 执行 |
| `media.apply-plan` | reversible-write | `plan.version:N` 绑定；**只交付不授权**——仅执行已获 Web 人工授权回执的计划，无人工回执时返回 `needs_approval`（零副作用） |
| `media.rollback` | reversible-write | `result.version:N` 绑定的受控回滚（撤销已授权执行结果的逆操作） |

`media.scan` / `media.retry` / `media.cancel` / `media.sync-external` 已在契约枚举中保留，尚未开放 handler。

## 验证

- 本地协议回放：`just check-capability`（schema 双端回放）。
- 隔离环境冒烟（mock DSH 调用链）：`just smoke-capability-compose`（需已构建镜像）。
- 真实 DSH 实例联调属延后验收项（见任务延后台账）。
