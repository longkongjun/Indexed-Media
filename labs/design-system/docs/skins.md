# 皮肤体系（skins）· 二维主题架构

> 版本：v2.0 ｜ 决策：D-0008 ｜ 机制参考：Material 3 tonal palette / dynamic color。
> 二维模型：**皮肤（`data-skin`）× 主题（`data-theme`）**——每套皮肤自带浅/暗两份完整语义值，全部通过 WCAG 2.2 AA（62 必检对 × 皮肤 × 双主题，`check-contrast.mjs` 强制门禁）。

## 1. 概念

| 维度 | 属性 | 含义 | 记忆 |
|---|---|---|---|
| 皮肤 skin | `data-skin`（缺省 = evergreen） | 配色家族 + 可选风格参数（radius/密度） | `localStorage['mfd-skin']` |
| 主题 theme | `data-theme`（缺省 = 跟随系统） | 该皮肤内的浅/暗两份语义值 | `localStorage['mfd-theme']` |

组合矩阵：N 皮肤 × 2 主题 = 2N 套语义值。皮肤"换皮不换骨"：语义结构（状态映射、单一主操作、双编码、六态语义）跨皮肤恒定；可变的只有颜色值与风格参数。

## 2. 内置皮肤

| id | 名 | 定位 | 来源 | 风格参数 |
|---|---|---|---|---|
| evergreen | 常青 | 绿色系管理台基线（产品默认） | 手工精调（tokens.json 基线） | 默认 radius |
| aurora | 极光 | 青蓝主色 + 冷中性；锐利密度 | skin-gen（OKLCH seed H240/C0.125） | radius 全档 −1px |
| sepia | 胶片 | 暖棕主色 + 暖中性；柔和手感 | skin-gen（H55/C0.105） | radius 全档 +2px |

## 3. 生成机制（M3 tonal palette 的零依赖实现）

`scripts/skin-gen.mjs`：

1. **seed**：每皮肤定义 7 个 palette 锚（primary/neutral/neutral-variant + success/warning/danger/info），各为 OKLCH 的 `Hue + Chroma`。
2. **tone 阶**：M3 的 tone（0-100 连续标度）经锚点插值映射为 OKLCH L（感知亮度），同 tone 跨色相亮度一致——这是换肤后对比度可预测的关键。
3. **语义槽位映射**（M3 scheme 精神）：固定规则如 light `primary=T48 / on-primary=T100 / surface=T99 / text-primary=T10`、dark `primary=T80 / surface=T14`……容器类用低饱和变体（C×0.5）。
4. **色域钳制**：保持 L/H 收缩 C 直至入 sRGB。
5. 产出 `tokens/skins/<id>.json`（入库定稿，非构建期依赖）；微调直接改 JSON 或调 seed 重跑。

对比度迭代：`check-contrast.mjs` 深合并皮肤层后按全组合验证；生成值不达标就调 tone（如 dark border-strong T44→T50 的修正记录）。

## 4. CSS 产物结构（build-tokens.mjs）

```css
:root, :root[data-skin=evergreen] { /* 基线浅色 + 主题无关 token */ }
[data-theme="dark"], [data-skin=evergreen][data-theme="dark"] { /* 基线暗色 */ }
@media (prefers-color-scheme: dark) { :root:not([data-theme]) { /* 跟随系统 */ } }
:root[data-skin=aurora] { /* aurora 浅色 + 风格参数（radius 覆盖） */ }
[data-skin=aurora][data-theme="dark"] { /* aurora 暗色 */ }
@media (prefers-color-scheme: dark) { :root[data-skin=aurora]:not([data-theme]) { … } }
```

防闪：首帧内联脚本同时恢复 `data-theme` 与 `data-skin`（见 site/index.html）。

## 5. 扩展新皮肤（流程）

1. 在 `skin-gen.mjs` 的 SKINS 表加 seed（7 锚 + label/desc/style）→ 运行生成 JSON；
2. `node scripts/check-contrast.mjs` 全绿（不绿调 tone 或 C）；
3. `node scripts/build-tokens.mjs` 重建 CSS；站点侧栏皮肤控件与皮肤圆点加一项；
4. 消费方（apps/web）复制 tokens.css 即得新皮肤，无需改组件代码。

## 6. 消费方映射

- **Web**：`data-skin` 属性 + tokens.css（组件零改动）；设置入口建议"设置 → 系统 → 外观"（皮肤 + 主题两组控件）。
- **Compose（M3）**：每皮肤一组 `MfdColor(light/dark)` 常量（从 skins JSON 固化）；或直接接 M3 dynamic color 体系（tonal palette 同构，tone 值可直接喂 `MaterialTheme`）。
- **SwiftUI**：皮肤 × 主题 = `MfdTheme` 枚举，经 Environment 注入；动态颜色按 (skin, traitCollection) 双参取值。
- **TV**：同皮肤体系，观影场景默认暗色 + 可选皮肤。

## 7. 约束（红线）

1. 皮肤不得改变语义结构（组件规格的状态映射/交互规则跨皮肤恒定）。
2. 每皮肤必须双主题双值且过 AA——不达标禁止发布。
3. 风格参数只开放白名单（radius/密度/图标风格类）；spacing/字体阶不随皮肤变（避免布局跳动）。
