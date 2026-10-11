# 会话消息（session-message）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：现有提取（app/AiSessionPanel.vue）+ 脱敏展示规则系统化（dsh-collaboration §4）
> AI 会话（DSH）的单条消息：脱敏摘要 + 可跳转核对 + 六态语义；**会话内无审批/执行按钮**。

## 1. 解剖

```text
┌ session-message[data-role=user|assistant|system] ─────────┐
│ [角色标] "你" / "AI 助手"（font-size-xs，text-secondary）    │
│ [正文]   消息内容（font-size-sm；用户消息底 selected，        │
│          AI 消息底 surface，靠对齐+角色标区分，不靠强对撞色）  │
│ [引用块]（可选）mono 井：相对路径/规则 ID/错误码             │
│ [引用链接]（可选）"任务 #1042 →"（text-link，深链核对）       │
│ [状态尾]（可选）六态尾注：进度/失败对象+原因/待办             │
└───────────────────────────────────────────────────────────┘
```

Token：用户消息底 `color-bg-selected`；AI 消息底 `color-bg-surface`；井 `color-bg-surface-sunken` + `font-family-mono`；链接 `color-text-link`；容器间距 `space-3`，圆角 `radius-lg`（消息内 `radius-md`）。

## 2. 状态 × 变体矩阵

变体：`role`（user/assistant/system）× 内容形态（文本/引用块/链接引用/状态尾注）。

| 会话六态 | 尾注表现 | 下一步 |
|---|---|---|
| Loading | ◌ 生成中（流式打点） | 无（不可取消的只读等待） |
| Empty | "没有查到…"+ 建议改写问题 | — |
| Error | 对象+原因+`required_action` 人话翻译 | 按翻译给出动作链接 |
| Offline | "AI 助手不可达，可先用页面功能" | 引导用 Web 功能 |
| Conflict | "依据/版本冲突，请重新查询" | 重新查询按钮 |
| Partial | 部分完成 + 待办清单 | 待办链接 |

双主题：全部色对双值成立；🔆 暗色见 §5。

## 3. 行为与交互（脱敏展示规则——本组件的核心约束）

1. **路径**：只显示相对路径或根目录 ID（`/media/电影/xx.mkv` → `电影/xx.mkv`）；完整宿主路径、令牌、凭据、原始媒体字节一律不出现（Core 侧脱敏为权威，UI 层再做一道）。
2. **错误**：展示结构化错误的稳定错误码 + `required_action` 人话翻译，不展示堆栈/内部 URL。
3. **可跳转**：凡引用任务/计划/媒体，必附深链（任务详情/plan-preview/媒体详情）——"结论必须可跳转核对"。
4. **审批**：审批相关消息只渲染 plan-preview 的 `summary` 变体 + 跳转链接，无确认按钮。
5. 事件投影消息（现 AiSessionPanel 模式）：`event_type` 中文标签 + `sequence` 序号，不渲染输入负载。
6. 输入框：多行自动增高（max 6 行）；发送中禁用；Enter 发送 / Shift+Enter 换行；无网络时发送按钮禁用并提示 Offline。

## 4. 可访问性

- 键盘：消息容器不聚焦（非交互）；引用链接与"重新查询"类按钮 Tab 可达；输入框标准文本框键盘行为。
- 焦点：新消息到达 `role=log` 区自动播报但不抢焦点。
- ARIA：会话列表 `role=log` `aria-live=polite`；角色标 `aria-hidden`（正文前缀文字已含）；引用块 `role=note`。
- 对比度：正文 text-primary × surface/selected 全过（contrast_pairs 已含 selected 对）；链接色双过；井内路径 text-primary × sunken 全过。
- 双编码：状态尾注图标+文字；角色区分靠角色标文字而非仅底色。

## 5. 主题表现（🔆）

暗色：用户消息 selected 为深绿 tint（与 AI 消息 surface 拉开明度而非色相对撞）；井为深井底；引用链接亮绿。媒体缩略引用（若含海报小图）遵循 dark-media-principles §1（不压暗）。Loading 打点动画暗色同参数。

## 6. 用法与反例

✅ AI："已找到 3 个候选，最高置信度 0.92（依据：TMDB 标题+年份），详见 任务 #1042 →"（链接核对）。
✅ 失败消息："查询任务 #1042 失败：连接超时。下一步：稍后重试或检查 DSH 连接。"
❌ 会话内出现"确认执行/批准"按钮（红线，审批一律跳 Web 全影响视图）。
❌ 消息内显示 `/Users/xxx/...` 完整路径或 token（脱敏红线）。
❌ 隐藏来源结论（"已整理完成"而无任务链接——不可核对）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 定位 | 全局侧栏抽屉 + 任务/媒体页上下文唤起 | M5 第二批评估（家庭成员只读权限严格小于管理者） | 同左 |
| 消息流 | role=log + polite | LazyColumn + liveRegion | ScrollView + accessibilityElement |
| 脱敏 | 同规则（UI 第二道） | 同 | 同 |

## 8. 变更记录

- v1.0：首版；吸收 AiSessionPanel 事件投影先例（TYPE_LABELS 应提取为共享字典——采纳时修复审计 S-3 债）。
