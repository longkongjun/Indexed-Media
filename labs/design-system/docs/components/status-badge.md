# 状态徽章（status-badge）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：现有提取（ScanStatusBadge ◆ 双编码 / .status-badge / 服务健康 ● ◐）
> 一切状态的最小展示单元：任务态、扫描态、连接健康态、事件态——形状符 + 中文标签双编码，永不单靠颜色。

## 1. 解剖

```text
┌ status-badge[data-tone=success|warning|danger|info|neutral] ┐
│ [形状符] ◆ ▲ ● ◐ ○ …（font-family 系统符号，aria-hidden）    │
│ [标签] 中文状态词（font-size-xs，semibold）                    │
└──────────────────────────────────────────────────────────────┘
```

- 底 `color-state-*-bg`、字 `color-state-*`（neutral：底 surface-sunken、字 text-secondary）；内边距 `space-1` 横 × 纵向紧贴（行高收紧）；圆角 `radius-full`（胶囊）或 `radius-xs`（表格紧凑）。
- 服务健康（概览服务状态行先例）：`●` 正常（success）/ `◐` 降级（warning）/ `○` 不可用（neutral，非 danger——连接失败不等于系统失败，可配"打开首页"外链）。

## 2. 状态 × 变体矩阵

| 语义 | 形状 | tone | 词表 |
|---|---|---|---|
| 成功/完成 | ◆ 或 ✓ | success | 完成、成功、已登记、正常 |
| 待处理/部分 | ◐ | warning | 待确认、部分成功、降级、待处理 |
| 失败/冲突 | ▲ 或 ✗ | danger | 失败、冲突、错误 |
| 进行中 | ◌ 或 → | info | 运行中、排队、计划中 |
| 中性/未开始 | ○ | neutral | 未开始、已停用、跳过 |

变体：`pill`（默认）/ `compact`（表格内，无底色仅形状+文字）/ `dot`（服务状态行）。

双主题：tone 色对（state-* × state-*-bg）双主题全过（contrast_pairs 文本组）。

## 3. 行为与交互

- 纯展示（非交互）；若整体可点跳筛选（如概览计数），用包裹链接并在徽章外加可点区域，徽章本身不加 hover 态。
- 词表与 wiki 术语表一致；扫描态部分成功沿用 `partial-success → 部分成功`（现状先例）。
- 事件类型徽章（capability 事件 11 类）：tone 映射——created/updated → info；failed → danger；needs_approval → warning；统一走共享 TYPE_LABELS 字典（修 S-3 债）。

## 4. 可访问性

- 键盘：非交互，无键盘路径（可点包裹链接由链接承担）。
- ARIA：形状符 `aria-hidden`；标签为真实文字（读屏直接读词）。
- 对比度：五 tone 文字对底全过 4.5（contrast_pairs）；compact 无底变体 = state-* × bg-surface 已验。
- 双编码：形状 + 文字是规范本体（不是可选项）。

## 5. 主题表现

不敏感（tone token 双值）；暗色下底为深 tint、前景为亮值，视觉重量与浅色一致。

## 6. 用法与反例

✅ 任务列表状态列：`◐ 部分成功`。
✅ 概览服务状态：`● TMDB` `◐ Jellyfin [打开首页]`（健康不产生任务计数——外链而非徽章跳任务）。
❌ 只有色块无文字（违反双编码）。
❌ "失败"用 warning 色（失败=danger，待处理=warning，语义不混用）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 实现 | span 全局类 | `AssistChip`（无交互）或自绘 | `Label` 自定义 |
| 形状符 | 系统字体符号 | Material 图标对应形 | SF Symbol 对应形 |
| 词表 | 共享字典 | 共享字典（KMP 层） | 共享字典（Facade 层） |

## 8. 变更记录

- v1.0：统一 ScanStatusBadge/服务健康/事件类型三类先例；tone 五值与词表对齐术语表。
