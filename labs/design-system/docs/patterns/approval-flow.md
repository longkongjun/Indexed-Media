# 交互模式 · 审批链端到端（approval-flow）

> 版本：v1.0 ｜ 基准：dsh-collaboration §2 典型流程 + capability 协议 approval-required 模式。
> 从"AI/系统提出计划"到"Core 执行留痕"的完整 UX 链；Web 是唯一权威审批面。

## 1. 端到端时序

```text
① 意图（会话或页面操作）
② DSH 经能力网关查询（read-only）→ 会话展示候选/计划摘要（session-message summary 变体）
③ 计划需要授权 → capability 返回 needs_approval → 会话给"到 Web 审批"深链 + 通知中心落一条"待审确认"
④ Web：approval-banner（任务详情/概览）→ plan-preview（full，完整影响清单）
⑤ 管理员核对 → 批准（二次确认，绑定 plan.version / expected_revision）或拒绝（给理由路径）
⑥ Core 执行 + journal 留痕 → Web 时间线实时推进（SSE）+ 会话同步结果消息
⑦ 结果：Success 确认 / 部分成功 → recovery-actions 模式
```

## 2. 分工红线（每步核对）

| 界面 | 允许 | 禁止 |
|---|---|---|
| 会话 | 摘要、解释、深链、结果播报 | 任何"确认执行"按钮；隐藏计划；复用过期引用 |
| plan-preview | 完整影响、批准/拒绝 | 摘要级快捷批准 |
| Core | 最终校验、执行、留痕 | —（UI 不可绕过 revision 防护） |

## 3. 异常路径

| 异常 | UX |
|---|---|
| 计划版本失效（revision 不匹配） | plan-preview 转 Conflict 态："计划已被重新计算"→ 查看新版本 |
| 审批中途 Offline | 提交禁用（按钮 disabled + 原因），批准请求幂等键保证重试不重复 |
| DSH 停机/超时 | 会话 Offline 态；**已批准计划不受影响**（Core 侧执行独立）；降级矩阵（A4）其余功能照常 |
| 会话展示与 Core 事实不一致 | 以 Web/Core 页面为准；会话消息附"以任务详情为准"核对链接 |

## 4. 通知与角标联动

- needs_approval → 通知中心"待审确认" + 任务导航角标计数 +1；批准/拒绝/过期 → 通知更新、角标 -1。
- 角标点击 → 任务列表筛选"待处理"；横幅/通知点击 → 直达 plan-preview。

## 5. 键盘完整路径（验收锚点）

Tab 依序：横幅主操作 → 计划头部 → 操作清单（可跳过行）→ 拒绝 → 批准；确认对话 Enter 默认"取消"；Esc 关闭对话不丢已核对状态（返回 plan-preview 顶部）。

## 6. 平台差异

Web 全量；移动端只读摘要 + 跳 Web（M5 第一批）；TV 不承载。深链格式：`/tasks/:id?plan=:version`。
