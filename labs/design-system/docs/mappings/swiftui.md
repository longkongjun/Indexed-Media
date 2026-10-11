# Token 映射 · SwiftUI（iOS / m5-client）

> 消费方：apps/ios（m5-client C3 及之后）。MFD 语义 token → 动态颜色（`UIColor` dynamic provider）或 Theme 环境值；Spacing/Radius/Typography → 常量枚举。

## 原则

1. 双主题用 `UIColor` 的 dynamic provider 一处定义（traitCollection 自适应），SwiftUI 侧 `Color(uiColor:)` 包装——系统浅/暗切换零额外代码；应用内手动切换（若提供）经自定义 Theme 环境值覆盖。
2. 值以 tokens.json v1.0.0 为准固化，文件头注释版本号；升级对照 manifest breaking_changes。
3. 暗色不压暗海报（AsyncImage 不加 overlay 暗化层；见 dark-media-principles）。

## 颜色定义（示意）

```swift
// MfdColors.swift — 值对照 tokens.json v1.0.0
extension Color {
  static let canvas = Color(uiColor: .init { tc in
    tc.userInterfaceStyle == .dark ? UIColor(hex: 0x0C1310) : UIColor(hex: 0xF4F7F5)
  })
  static let surface     = dynamic(0xFFFFFF, 0x121B16)
  static let ink         = dynamic(0x12231C, 0xE6EDE8)   // text-primary
  static let secondary   = dynamic(0x50655C, 0x9FB3A7)   // text-secondary
  static let primaryAction = dynamic(0x08744B, 0x4CC38A)
  static let onAction    = dynamic(0xFFFFFF, 0x0A1410)
  static let success     = dynamic(0x0D5C30, 0x58C98D)
  static let warning     = dynamic(0x7D5F0E, 0xD9A53F)
  static let danger      = dynamic(0x8B2A2A, 0xE57373)
  static let info        = dynamic(0x1D5FA8, 0x6AA9E8)
  // …完整清单按 tokens.json 逐条对应，此处不列全
}
```

## 系统语义槽对应

| MFD token | SwiftUI 惯用 |
|---|---|
| color-bg-canvas / surface | `Color.canvas` / `Color.surface`（不用 `.background` 系统色——MFD 自带绿倾向） |
| color-text-primary | `.foregroundStyle(.ink)`（不用 `.primary`——保留系统色给纯平台组件） |
| color-action-primary | 主按钮填充色（不用 `.tint` 直接默认化，避免影响系统控件色彩预期；需统一时 `tint(.primaryAction)`） |
| color-border-strong / default | `Rectangle().strokeBorder(.lineStrong)` / `.line` |
| color-state-*（success 等） | 徽章/横幅专用前景色，不映射到系统 `green/orange/red` |

## 尺寸/字体/动效

- 间距：`enum MfdSpacing { static let s1: CGFloat = 4 … s16: CGFloat = 64 }`（对应 space-1..16）。
- 圆角：`MfdRadius.sm=6 .md=8 .lg=12 .xl=16`；胶囊用 `.capsule`。
- 字体阶梯映射系统 Dynamic Type：body=16pt 对 `.body`、表格 14pt 对 `.subheadline`、标题 20/25pt 对 `.title3/.title2`；**不锁死字号**——用相对系统样式，阶梯作为视觉校准基准而非覆盖 Dynamic Type；等宽路径用 `.monospaced()` 修饰。
- 动效：`.animation(.easeOut(duration: 0.12))` 微交互 / `0.24` 容器；主题切换过渡由系统 trait 动画承担；尊重"减弱动态效果"（`accessibilityReduceMotion`）。
- 触控：`44×44` 最小热区（`.frame(minWidth: 44, minHeight: 44)` 配 `.contentShape`）。

## 验收锚点

- 双主题（模拟器 Light/Appearance 切换）截图对照风格指南站 demo；
- Dynamic Type 放大到 AX 档不破版（消费方 UI 测试项）。
