# 通知（notification）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：现有提取（NotificationsView + useNotifications）
> 站内通知中心的通知行与页面模式：review.waiting 触发、分页、未读语义、任务跳转。

## 1. 解剖

```text
┌ notification-item[data-kind][.unread] ─────────────────────┐
│ [左条纹]（unread 时 border-strong 色 inset 条）              │
│ [kind 胶囊] 通知类型（xs，底 selected，如"待审确认"）         │
│ [标题] 一句话（font-size-sm，semibold）                      │
│ [正文] 详情（text-secondary，sm）                            │
│ [时间] <time>（xs，text-muted，相对时间 + title 绝对时间）    │
│ [操作列] "查看任务"（link）· "标记已读"（secondary compact） │
└────────────────────────────────────────────────────────────┘
```

页面骨架 = table-list 的 `list` 变体 + 工具栏（"只看未读"checkbox + "全部标记已读"，后者在 `unreadCount===0` 时禁用——现状先例保留）。

Token：未读条纹 `color-border-strong`；kind 胶囊底 `color-bg-selected` 字 `color-text-secondary`；已读行底 surface、未读行底 surface + 条纹（不用大面积底色差）。

## 2. 状态 × 变体矩阵

| 状态 | 表现 |
|---|---|
| 未读 | 左条纹 + 标题 semibold |
| 已读 | 无条纹 + 标题 regular + 整行 text-secondary 倾向 |
| 标记中 | 操作按钮 loading（防重复） |
| 空 | state-container Empty："暂无通知" + 说明何时产生通知 |

kind 词表：待审确认（review.waiting）/ 系统公告 / 连接异常（预留）——按 Core 触发器扩展，新 kind 须同步共享字典。

双主题：全部 token 双值成立。

## 3. 行为与交互

- 逐条"标记已读"独立请求，失败行内重试提示（不清列表）；"全部标记已读"完成后刷新计数（导航角标）。
- "查看任务"跳任务详情并**自动标记该条已读**（跳转即读，同标签页历史）。
- 分页 CursorPager；时间倒序。
- 角标计数 = 未读数，出现在导航（见 navigation）；计数只统计需要管理员介入的类别。

## 4. 可访问性

- 键盘：两操作按钮 Tab 序（查看在前、已读在后）；checkbox 标准行为。
- 焦点：标记已读后焦点留在该行（列表不重排；或行移除时焦点移下一条——以不丢焦点为准）。
- ARIA：列表 `role=list`；未读状态用文字（"未读"title 或 sr-only 文本）不只靠条纹；时间 `<time datetime>`。
- 对比度：标题/正文文字对全过；kind 胶囊 secondary × selected 全过；条纹 border-strong 识别边界级（双过 3:1）。
- 双编码：kind 胶囊文字 + 未读条纹附 sr-only "未读"。

## 5. 主题表现

不敏感；暗色 kind 胶囊为深绿 tint、条纹亮描边色。

## 6. 用法与反例

✅ 审批等待通知：kind"待审确认" + 标题"任务 #1042 等待审批" + "查看任务"直达。
❌ 通知正文含完整路径/凭据（脱敏规则同 session-message）。
❌ 用模态弹窗逐条推送通知（通知中心聚合浏览，不打断）。
❌ 已读/未读只靠字重区分（需条纹 + sr-only 双保险）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 中心页 | /notifications | Screen（M5 第一批可含只读） | View |
| 推送 | 不做系统推送（站内） | 预留（第二批评估） | 预留 |
| 角标 | 导航角标 | BottomBar badge | TabView badge |

## 8. 变更记录

- v1.0：规格化现状实现；明确"跳转即读"、角标计数口径、kind 字典共享（超前注释的 Webhook 管理区块列为预留，不入本版）。
