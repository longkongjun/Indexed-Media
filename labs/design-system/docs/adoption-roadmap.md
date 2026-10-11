# 三方采纳路线图（adoption-roadmap）

> 版本：v1.0 ｜ 消费方：apps/web（主线）、m5-client 双端、未来 TV。
> 原则：设计规范生产者已完成交付；各消费方按自身节奏采纳，互不阻塞。所有产物入口：`labs/design-system/`（worktree `feature/design-system`）。

## 1. apps/web（主路径：CSS custom properties 直接替换）

**为什么不走 Tailwind**：apps/web 现状是手写 CSS 变量方案（审计 D-0002），token 替换零迁移成本；Tailwind 4 迁移是独立的技术栈决策，不应与设计系统采纳绑定。

### 阶段 A：token 层替换（预计 0.5–1 天）

1. 复制生成物 `site/assets/tokens.css` 进 `apps/web/src/styles/`（替换现 tokens.css；`--mfd-` 前缀与现 6 变量并存期可选：先加新变量，再逐文件切换引用）。
2. 全局替换映射（机械替换）：`--color-ink→--mfd-color-text-primary`、`--color-muted→--mfd-color-text-secondary`、`--color-surface→--mfd-color-bg-surface`、`--color-canvas→--mfd-color-bg-canvas`、`--color-brand→--mfd-color-bg-inverse`、`--color-accent→--mfd-color-action-primary`、`--color-focus→--mfd-color-border-focus`、`--color-danger→--mfd-color-state-danger`、`--border→--mfd-color-border-default`、`--radius→--mfd-radius-lg`、`--shadow→--mfd-shadow-2`。
3. 消灭散落硬编码（审计 S-1~S-7 债）：琥珀 `#98771a`→`--mfd-color-state-warning(-border)`；绿系拼写变体 6 处→对应语义 token；`.sr-only`/`.visually-hidden` 归一；TYPE_LABELS 提共享字典。
4. 验收：grep 无残留 hex（scoped style 内）；现有页面视觉无回归（人工过一遍 23 路由）。

### 阶段 B：暗色主题上线（预计 0.5 天）

1. index.html 加防闪内联脚本 + 设置页"外观"三态控件（theme-switching 模式，代码可从风格指南站 site/ 直接抄）。
2. scoped style 内写死深色侧栏色的组件（AiSessionPanel 等）换 inverse 系 token。
3. 验收：三态切换 + 刷新不闪 + prefers-reduced-motion。

### 阶段 C：组件规格对齐（随功能迭代渐进，无独立工期）

- 新页面/新组件（审批横幅、计划预览、任务时间线）直接按组件规格实现（规格即需求）。
- AsyncState 扩展 conflict/partial 分支（G-9）随 A4 审批链 UI 落地。
- 既有页面样式向规格对齐（form-card 配方、表头底、徽章 tone）随手做，不立专项。

**建议时点**：阶段 A+B 在主线下一批 UI 工作（A4 审批链 UI / P5）开工前完成——审批横幅等新组件直接在 token 体系上写，避免二次返工。

## 2. m5-client（batch2+ 消费）

**第一批不采纳**（C2 Compose 六屏按 M3 默认主题交付，避免批次范围膨胀）；第二批起：

1. `MfdColor.kt` / `Color+Mfd.swift` 按映射表固化（值锚定 tokens v1.0.0 + 版本注释）；Compose 建扩展槽。
2. 六屏重着色 + status-badge/state 语义对齐（词表共享字典入 KMP commonMain）。
3. iOS 同步（动态颜色方案直接支持双主题）。
4. 验收：同 fixtures 双端截图对照风格指南站 demo（cross-platform-matrix §3）。

**预计**：Android 1–1.5 天 + iOS 1 天（不含新功能）。**建议时点**：C2 开工时直接带上（成本最低）；否则第二批首个 UI 任务时。

## 3. 未来 TV（M6，适配原则消费）

- 直接消费暗色偏好（观影环境默认暗色合理）；token 经 Compose for TV / SwiftUI 层同款映射。
- 焦点块 ≥48dp 替代触达规范；管理面组件（审批/恢复/配置）不实现。
- 无须提前工作；立项时从 mappings + cross-platform-matrix §4 起步。

## 4. ui-contract（packages/，另案）

本任务输出（tokens.json / 状态语义 / 覆盖矩阵）可作 ui-contract 初始化输入——它 README 自述职责与本任务产出域一致。属 packages/* 变更，不入本任务；建议主线或独立小任务处理（0.5 天：复制 tokens.json + 引用规范文档 + check 脚本接入）。

## 5. 升级与破坏性变更

- tokens/规格带版本（manifest.json）；破坏性变更记录 breaking_changes 字段，消费方按版本对照评估。
- 风格指南站常驻 `labs/design-system/`，采纳前后对照与回归核查均以它为准。
