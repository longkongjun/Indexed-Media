# 审批横幅（approval-banner）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：新功能规格（A4 approval-required 编排闭环）
> 告知"有计划等待审批"并给出**唯一权威审批入口**；会话内不出现审批按钮（dsh-collaboration §4 红线）。

## 1. 解剖

```text
┌ approval-banner[role=alert] ─────────────────────────────┐
│ [图标] ⚖ 审批图标（icon-size-md）                          │
│ [标题] "1 个整理计划等待审批"（font-size-base，semibold）    │
│ [摘要] 计划一句话摘要：对象 + 操作类型 + 数量（text-secondary）│
│ [计划版本] plan v<version>（mono，text-secondary）          │
│ [主操作] "查看完整计划并审批" Button(primary) → plan-preview │
│ [次操作] "稍后处理" link-button（仅收起到通知）              │
└──────────────────────────────────────────────────────────┘
```

Token：底 `color-state-info-bg`、描边 `color-state-info-border`（识别边界级）、标题 `color-text-primary`、摘要 `color-text-secondary`、版本号 `font-family-mono`。

放置：任务详情页顶部（该任务的计划）/ 概览"需要处理"区（聚合入口）。**不在会话消息内联出现**。

## 2. 状态 × 变体矩阵

变体：`task`（任务详情内）/ `overview`（概览聚合，可多个）。

| 状态 | 表现 | 主操作 |
|---|---|---|
| 等待审批（默认） | 如上解剖 | 查看完整计划并审批 |
| 审批中（用户已打开计划） | 摘要更新为"计划核对中"，主操作禁用防重复进入 | — |
| 已批准 | 横幅转 Success 确认（state-container Success 态），显示计划版本与执行去向 | 查看执行结果（跳任务时间线） |
| 已过期（plan version 失效） | 转 Conflict 态：计划已被重新计算，旧审批不可用 | 查看新版本计划 |
| 已拒绝 | 转中性态收起，任务回待确认/冲突态 | 返回任务详情 |

双主题：全部色对（text-primary × info-bg；info-border × surface）双主题成立。

## 3. 行为与交互

- **版本绑定**：主操作携带 `plan.version`（`expected_revision`）；审批页校验版本不一致即转"已过期"态（Core 侧 revision 防护为权威，UI 如实反映）。
- **审批不做横幅内快捷确认**：批准/拒绝只能在 plan-preview 全影响视图完成（dsh-collaboration：会话/摘要界面不出现"确认执行"）。
- "稍后处理"仅将横幅收起并写一条通知，不改变待审批状态；任务导航角标计数不变。
- 审批完成后横幅不可再操作，保留为结果记录（journal 留痕的 UI 对应物）。

## 4. 可访问性

- 键盘：主/次操作原生 button；横幅出现时若为页面首要事件，容器 `tabindex=-1` + focus。
- 焦点：focus-visible 环（`color.border-focus`，stroke-focus-ring，全局基线）。
- ARIA：`role=alert`（出现时播报标题）；状态转换后 `aria-live=polite` 更新；版本号文本纳入播报。
- 对比度：text-primary × info-bg、info-border × bg-surface 均入 contrast_pairs（文本/非文本组）双主题全过；info-border 为识别边界级。
- 双编码：⚖ 图标 + "等待审批"文字标题，不靠底色区分。

## 5. 主题表现

不敏感（info 系深浅底双值已验）；暗色下 info-bg 为深蓝黑 tint，图标与标题前景取 info 亮值，无额外规则。

## 6. 用法与反例

✅ 任务详情页：任务停在设计划阶段 → 横幅展示计划 v7 摘要 → 跳全影响视图审批。
✅ 概览"需要处理"区聚合 2 个待审批 → 每条独立横幅（overview 变体）。
❌ 横幅内放"一键批准"（未展示完整影响即执行，违反"任何文件影响先展示来源、目标、操作类型"原则）。
❌ 会话消息里渲染此组件的批准操作（红线）。
❌ 版本失效后仍允许按旧摘要批准（必须经"查看新版本"重绑）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 组件 | div[role=alert] + 全局类 | `Banner`（M3 新组件；扩展槽色） | 自定义 View + `.accessibilityElement` |
| 放置 | 页面顶部内容流 | `Column` 首项 | `VStack` 首项 |
| 深链 | router push plan-preview | Navigation | NavigationLink |

## 8. 变更记录

- v1.0：首版（对齐 capability 协议 approval-required 模式与 A4 降级矩阵语义）。
