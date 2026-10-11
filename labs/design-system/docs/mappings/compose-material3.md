# Token 映射 · Compose Material 3（Android / m5-client）

> 消费方：apps/android（m5-client C2 及之后）。MFD 语义 token → `MaterialTheme.ColorScheme` + 自定义扩展槽；间距/圆角/字体 → `MaterialTheme` 对应槽或常量对象。

## 原则

1. **语义对齐优先于逐字面映射**：MFD token 名即语义，Kotlin 侧建 `MfdColor` 对象（light/dark 两个实现），`MaterialTheme` 的槽位吃 MFD 值；Material 3 没有对应槽的（成功/警告/信息三态、侧栏组、图表），放扩展槽（`LocalMfdExtendedColors`）。
2. **双主题**：`isSystemInDarkTheme()` 选择 light/dark 两套 `MfdColor`；应用内主题切换（若提供）叠加用户偏好状态。暗色不压暗海报（Coil 加载图不做 color filter）。
3. **不自建调色板**：值以 tokens.json v1.0.0 为准一次性固化进 `MfdColor.kt`（附版本注释），升级时对照 manifest 的 breaking_changes 评估。

## ColorScheme 映射表

| MFD token（light / dark） | M3 槽 |
|---|---|
| color-bg-canvas `#f4f7f5` / `#0c1310` | `background` |
| color-bg-surface `#ffffff` / `#121b16` | `surface` |
| color-bg-surface-raised / `color-bg-surface-sunken` | `surfaceVariant` / `surfaceContainerLowest`（按 M3 版本取就近槽） |
| color-bg-inverse `#10251d` / `#0a100d` | `inverseSurface` |
| color-text-primary | `onSurface` |
| color-text-secondary `#50655c` / `#9fb3a7` | `onSurfaceVariant` |
| color-text-inverse | `onInverseSurface` |
| color-action-primary `#08744b` / `#4cc38a` | `primary` |
| color-text-on-action `#ffffff` / `#0a1410` | `onPrimary` |
| color-action-secondary-bg | `surfaceVariant`（描边按钮自绘） |
| color-action-danger + on-danger | `error` / `onError` |
| color-border-strong | `outline` |
| color-border-default | `outlineVariant` |
| color-bg-selected | `secondaryContainer` |

## 扩展槽（M3 无对应）

```kotlin
// 伪代码示意，完整值对照 tokens.json
data class MfdExtendedColors(
  val success: Color, val successBg: Color,
  val warning: Color, val warningBg: Color, val warningBorder: Color,
  val info: Color, val infoBg: Color,
  val dangerState: Color, val dangerBg: Color, val dangerBorder: Color,
  val link: Color, val mediaPlaceholder: Color,
  val chart1..chart5: Color, val chartGrid: Color, val chartAxis: Color,
)
val LocalMfdExtendedColors = staticCompositionLocalOf<MfdExtendedColors>()
```

状态徽章语义：success/warning/danger/info 四态 + partial 复用 warning + running 复用 info（与 Web 一致，见组件规格"状态徽章"）。

## 尺寸/字体/动效

- 间距：`MfdSpacing` 对象（`xs=4.dp … xl=64.dp`，对应 space-1..16）；列表行 `space-3`、卡片内 `space-4`、卡片间 `space-6`。
- 圆角：`MfdRadius`（sm=6.dp md=8.dp lg=12.dp xl=16.dp full=CircleShape）。
- 字体：`Typography` 中 `bodyLarge=16sp/1.5`、`bodyMedium=14sp/1.5`、`titleLarge=25sp/1.25`、`titleMedium=20sp/1.25`；等宽路径文字用 `FontFamily.Monospace` 包装的专用 `MfdCodeTextStyle`。
- 动效：`tween(120)` 微交互 / `tween(240)` 容器；主题切换交叉淡入 400ms 并检查 `LocalAccessibilityManager` 减弱动效设置（对齐 Web `prefers-reduced-motion` 语义）。
- 触控：最小 44.dp（icon-size-touch-min），高于 M3 默认 48.dp 时不放宽、以 44.dp 为下限 48.dp 为推荐。

## 验收锚点

- 双主题截图对照风格指南站同组件 demo（D4 交付后可查）。
- 对比度：沿用 tokens.json contrast_pairs 结果，不另建标准。
