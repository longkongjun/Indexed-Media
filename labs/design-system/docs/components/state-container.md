# 六态容器（state-container）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：现有提取（apps/web components/AsyncState.vue + asyncState.ts）扩展至全六态
> 承载页面/区块级状态：Loading / Empty / Error / Offline / Conflict / Partial Success（+ Success 确认），统一"状态→展示重点→唯一主操作"结构。

## 1. 解剖

```text
┌ state-container[data-state] ────────────────────────────┐
│ [图标区]   状态图标（icon-size-lg，形状双编码）             │
│ [标题]     状态一句话（font-size-lg，semibold）             │
│ [正文]     对象+原因+下一步（font-size-sm，text-secondary） │
│ [主操作]   唯一 Button(primary)（可选）                     │
│ [次操作]   Button(secondary) 链接式（可选，最多 1）          │
│ [stale 区] stale=true 时底层内容以降低对比呈现并保留滚动位置  │
└──────────────────────────────────────────────────────────┘
```

Token 引用：容器底 `color-bg-surface`、内边距 `space-6`、圆角 `radius-lg`、图标色按态取 `color-state-*` 或 `color-text-muted`、正文 `color-text-secondary`；stale 区内容统一 `opacity: 0.55` 且**禁用全部交互**（`inert` + `pointer-events: none`）。

## 2. 状态 × 变体矩阵

变体：`page`（整页）/ `panel`（卡片内）/ `inline`（列表尾/行内）。

| 状态 | 图标（形状双编码） | 底色 | 主操作 | stale 共存 |
|---|---|---|---|---|
| Loading | 旋转环 ◌（aria-busy） | surface；骨架屏见 §3 | 无（取消操作仅在用户主动发起的加载提供） | 允许（默认） |
| Empty | 空盒 ▢ + 空态文案 | surface | "创建/配置"类下一步 | 不适用 |
| Error | ⛇ 感叹号 | `color-state-danger-bg` | 重试（primary） | 允许 |
| Offline | ⚡ 断线 | surface + `color-state-warning-border` 描边 | 重连/重试 | 允许（默认） |
| Conflict | ⚖ 双箭头相抵 | `color-state-warning-bg` | 跳冲突详情（见 plan-preview） | 允许 |
| Partial | ◐ 半圆 | `color-state-warning-bg` | 只重试未完成部分 | 允许 |
| Success | ✓ | `color-state-success-bg` | 完成（关闭/返回） | 不适用 |

双主题：矩阵在浅色/暗色均成立（底色 token 双值已过对比度验证）；🔆 暗色差异见 §5。

## 3. 行为与交互

- **Loading 延迟**：数据到达 <300ms 不闪 Loading（静默等待）；≥300ms 出骨架或旋转符；骨架屏 = 内容形状的灰块（`color-bg-surface-sunken`），脉冲动画 `motion-duration-base`，`prefers-reduced-motion` 时静态。
- **stale 语义**：刷新失败但已有旧数据 → Error/Offline 态 + stale 内容共存（旧数据可读不可操作）；空数据时无 stale，纯态全屏。
- **Error 重试**：重试按钮触发数据层 retry；重试期间回到 Loading（保留 stale）。
- **Offline 判定**：网络层错误（对齐 apiErrors `isOffline`）；全局 Offline 由导航层横幅承载，区块级仅在该区块依赖的服务不可达时出现。

## 4. 可访问性

- 键盘：主/次操作为原生 button（Tab 可达、Enter/Space 触发）；stale 区 `inert` 移出 Tab 序。
- 焦点：Error/Offline 出现时若打断任务流，将容器设 `tabindex=-1` 并 `focus()`（不抢 `role=alert` 的播报）。
- ARIA：容器 `role=status`（Loading/Offline/Success）或 `role=alert`（Error/Conflict）；Loading 时宿主区 `aria-busy=true`；图标 `aria-hidden=true`（文字已表达）。
- 对比度：正文对（color-text-secondary × state-*-bg）已入 contrast_pairs 文本组全过；Offline 描边为**识别边界**（border 分级：强制 3:1，`color-state-warning-border` 已验）。
- 状态不单靠颜色：每态有形状图标 + 中文标题（设计原则 3）。

## 5. 主题表现（🔆）

暗色要点（dark-media-principles §3）：Error/Conflict/Partial 底用深色 tint（state-*-bg 暗色值）避免大面积亮色底刺眼；Offline 横幅底用 `color-bg-surface` 而非 warning-bg；骨架屏底 sunken 深值。Success 确认不自动消失（两主题一致）。

## 6. 用法与反例

✅ 任务列表接口失败且已有数据：Offline 态 + stale 列表，重试后恢复。
✅ 媒体页首次进入为空：Empty 说明"尚无正式媒体记录" + 主操作"查看待确认任务"。
❌ 用 Empty 代替 Error（"没有数据"≠"取不到数据"，违反 web-experience §7）。
❌ Loading 态放置取消按钮之外的其他操作（误导用户可作用于未加载数据）。
❌ Conflict 态提供"强制继续"按钮（绕过授权路径，红线）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 实现 | AsyncState 扩展 conflict/partial 分支 | `Crossfade` + sealed class UiState | `switch` over enum + `.overlay` |
| 骨架屏 | CSS 灰块 | `Modifier.shimmer`/占位 Box | redacted placeholder |
| stale | `inert` + opacity | `Alpha(disabled=true)` | `.disabled` + opacity |
| Offline 全局横幅 | 导航层（见 navigation） | `Scaffold` snackbarHost 顶部 | safeArea 顶部条 |

## 8. 变更记录

- v1.0：从 AsyncState（loading/empty/error/offline + stale）扩展六态全集；新增 Conflict/Partial/Success 三态定义。
