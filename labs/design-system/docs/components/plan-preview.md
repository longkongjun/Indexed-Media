# 计划变更预览（plan-preview）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：新功能规格（A3 create-plan/apply-plan、审批链的完整影响视图）
> 审批前的**完整文件影响清单**：来源、目标、操作类型、冲突结果；这是 Web 作为"事实界面"的核心组件。

## 1. 解剖

```text
┌ plan-preview ────────────────────────────────────────────┐
│ [头部] 计划 v<version> · 对象任务 <task-id> · 生成时间      │
│ [摘要行] N 项操作：移动 a · 改名 b · 隔离 c · 写 NFO d      │
│ [操作清单] plan-op-row × N（按目录分组，组头 = 目标目录 mono）│
│  ├ [操作类型徽章] move/rename/quarantine/nfo（形状双编码）  │
│  ├ [来源] 相对路径（mono，可复制）                          │
│  ├ [→]                                                    │
│  ├ [目标] 相对路径（mono，可复制）                          │
│  └ [冲突/风险标记]（可选，warning 系）                       │
│ [底部操作区] 拒绝(secondary-danger) · 批准(primary)         │
│  └ 批准按钮文案绑定版本："批准并执行计划 v7"                  │
└──────────────────────────────────────────────────────────┘
```

Token：容器 `color-bg-surface` + `radius-lg` + `shadow-2`；操作徽章按类型取 state-*（见 §2）；路径 `font-family-mono` + `color-bg-surface-sunken` 井底；分组头 `color-text-secondary` + `border-subtle` 分隔。

## 2. 状态 × 变体矩阵

变体：`full`（审批视图）/ `summary`（会话/横幅内的一句话摘要，不含操作按钮）。

| 操作类型 | 徽章 | 形状 | 色系 |
|---|---|---|---|
| move（移动/跨盘） | "移动" | → | info |
| rename（改名） | "改名" | ✎ | info |
| quarantine（隔离） | "隔离" | ⚑ | warning |
| nfo（写 NFO） | "NFO" | ▤ | success |
| 冲突行（目标已存在等） | 行内 warning 标记 + 原因 | ⚠ | warning |

清单状态：`normal`（可批准）/ `conflicted`（含冲突行——批准禁用，先解决冲突）/ `stale`（版本失效——转 state-container Conflict 态）。

双主题：徽章/行底色 token 双值全过对比；路径井底 sunken 双值。

## 3. 行为与交互

- 路径一律**相对路径或根目录 ID**（脱敏规则同 session-message §3）；点击路径复制完整安全标识。
- 长清单（>50 项）默认折叠为分组计数，展开组查看明细；批准作用整个计划版本，不支持部分勾选（部分重试属执行后语义，见 task-timeline）。
- 批准 = 二次确认不可省略：按钮文案含版本号，点击后进入确认对话（"将执行 N 项文件操作，此操作经 journal 可回滚"）。
- 拒绝 = 计划作废回任务待确认态；给出"修改规则后重新计算"的次操作链接。

## 4. 可访问性

- 键盘：清单为列表（ul/li）；批准/拒绝 Tab 序在清单之后（先读影响再操作）；确认对话内 Esc = 取消、Enter 默认焦点在"取消"（防误批）。
- 焦点：打开审批视图时头部容器聚焦；确认对话焦点陷阱。
- ARIA：操作清单 `role=list`；徽章 `role=img` + aria-label（"移动操作"）；批准按钮 `aria-describedby` 指向摘要行。
- 对比度：徽章文字对（state-* × state-*-bg）双主题全过；分组分隔 border-subtle 为**装饰级**（豁免，D-0006）。
- 双编码：操作类型徽章形状+文字；冲突行 ⚠ + 文字原因。

## 5. 主题表现（🔆）

路径井（mono 块）在暗色为深井底（sunken 暗色值）+ 亮前景；冲突行 warning 系暗色值克制使用（dark-media-principles §3）；**清单不因主题改变操作类型色系映射**（同一计划在两主题下语义一致）。

## 6. 用法与反例

✅ 批准前逐行可见"来源 → 目标 + 类型"；冲突行阻止批准并引导修正规则。
✅ summary 变体在会话消息中展示"计划 v7：3 移动 · 1 隔离"并附深链跳本视图。
❌ 只显示"N 项操作"摘要就提供批准（影响不可见）。
❌ 把 hardlink 失败静默降级为复制并展示为"移动"（如实展示操作类型，红线）。
❌ 批准按钮放在清单上方且样式强于清单（先看后做的顺序暗示）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 清单 | ul + 分组 | LazyColumn + sticky header | List + Section |
| 路径复制 | clipboard API + 行内提示 | LocalClipboard | UIPasteboard |
| 审批入口 | 仅本组件 full 变体 | 同（审批属管理面，M5 第一批仅查看摘要） | 同左 |

> 注：M5 第一批移动端**只读**消费 summary 变体（跳转 Web 审批），批准操作不出现在移动端——与"Web 权威审批"一致。

## 8. 变更记录

- v1.0：首版（对齐 organization plans/quarantine 操作类型与 revision 防护语义）。
