# 组件清单（v1.0 定案）

> D2 覆盖最小集 12 件 + 复用说明件 2 项。状态：现有提取 = 从 apps/web 既有实现规格化；新功能 = 为 A4 审批链/通知/AI 会话等新能力预定义规格。规格文件按 slug 命名。

| # | 组件 | slug | 来源 | 主题敏感(🔆) | 说明 |
|---|---|---|---|---|---|
| 1 | 按钮与操作 | `button` | 现有提取（.primary-action / .cursor-pager button 等） | — | 主/次/危险/链接按钮 + 尺寸 + 加载态 |
| 2 | 输入与表单 | `form` | 现有提取（.form-card / .check-field / ErrorSummary） | — | 文本/选择/复选/单选 + 字段级错误 + 表单卡片 |
| 3 | 表格与列表 | `table-list` | 现有提取（activity-table / notification-list / CursorPager） | — | 数据表 + 列表行 + 游标分页 |
| 4 | 媒体卡片 | `media-card` | 现有提取（MediaArtwork）+ 媒体墙扩展 | 🔆 | 海报卡/墙格/角标/占位（暗色重点） |
| 5 | 状态徽章 | `status-badge` | 现有提取（ScanStatusBadge / status-badge） | — | 任务/扫描/健康状态，形状+文字双编码 |
| 6 | 任务时间线 | `task-timeline` | 新功能规格（任务详情阶段时间线） | — | 阶段→状态→单节点展开 |
| 7 | 计划变更预览 | `plan-preview` | 新功能规格（A3 create-plan / apply-plan 明细） | 🔆 | 文件操作清单 diff（移动/改名/隔离） |
| 8 | 审批横幅 | `approval-banner` | 新功能规格（A4 approval-required） | — | 审批入口 + 计划版本绑定 + 不可绕过 |
| 9 | 会话消息 | `session-message` | 现有提取（AiSessionPanel）+ 脱敏规则系统化 | 🔆 | AI 会话消息（脱敏/跳转/六态） |
| 10 | 通知 | `notification` | 现有提取（NotificationsView） | — | 通知行 + 中心页模式 + 未读语义 |
| 11 | 导航 | `navigation` | 现有提取（AppShell）+ 响应式三档 | — | 侧栏/底部导航/页签/角标计数 |
| 12 | 六态容器 | `state-container` | 现有提取（AsyncState）扩展至全六态 | 🔆 | Loading/Empty/Error/Offline/Conflict/Partial/Success |

复用说明件（不算独立组件，并入相关规格）：

- **骨架屏**：并入 `state-container`（Loading 形态）。
- **海报角标/遮罩**：并入 `media-card`。
- **等宽路径片段**（mono-path）：并入 `form`（只读展示）与 `plan-preview`；全局规则见 `session-message` §脱敏。

预留（不在 D2 范围，登记待消费方提需求）：Webhook 管理区块（通知中心注释超前实现处）、备份/恢复面板（P4.2c）、图表容器（ECharts 主题化，token 已备 chart-* 系列）。
