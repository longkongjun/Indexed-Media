# 按钮与操作（button）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：现有提取（.primary-action / 次按钮全局类 / link-button）
> 操作入口的唯一组件族：主/次/危险/链接四型 × 三尺寸 × 五状态。

## 1. 解剖

| 型 | 底 | 文字/图标 | 边框 | 用途 |
|---|---|---|---|---|
| primary | `color-action-primary` | `color-text-on-action` | 无 | 每视图**最多一个**的主动作 |
| secondary | `color-action-secondary-bg` | `color-text-primary` | `color-border-strong`（识别边界级） | 次要动作 |
| danger | `color-action-danger` | `color-text-on-danger` | 无 | 破坏性确认 |
| ghost/link | 透明 | `color-text-link` | 无 | 行内跳转/低强调操作 |

几何：高 `icon-size-touch-min`（标准）/ `space-8`（紧凑表格内，仍 ≥ 24px 目标（WCAG 2.5.8 下限）+ 周围留白）；内边距横 `space-3` 纵 `space-2`；圆角 `radius-md`；图标 `icon-size-md` + 与文字间隙 `space-1`。

## 2. 状态 × 变体矩阵

| 状态 | primary | secondary | danger | link |
|---|---|---|---|---|
| 默认 | primary | 白底/描边 | danger | link 色 |
| hover | primary-hover | 底 `color-bg-hover` | danger-hover | 下划线 |
| active | primary-active | 边框加深 | 同 hover | 同 hover |
| focus-visible | 焦点环（border-focus，stroke-focus-ring 宽，offset 同宽）——四型一致 | | | |
| disabled | `opacity .55` + `cursor:not-allowed` + **保持可读对比**（不降到 4.5 以下） | | | 灰（text-muted） |
| loading | 旋转符替代图标 + 文案保留 + 宽度锁定防跳 | | | 不适用 |

双主题：全部型色对双值已过（on-action × primary/hover；on-danger × danger）；暗色 primary 为亮绿底+深字（D-0005 语义双值）。

## 3. 行为与交互

- 主按钮每屏一个（设计原则 2）；连续确认对话中 Enter 默认绑定**取消**而非主按钮（防误触破坏性操作）。
- danger 型必须经确认对话（对象+影响+不可自动撤销说明）；不提供"不再询问"。
- loading 期防重复提交（disabled + 幂等键由数据层负责）；完成即恢复或整块转结果态（state-container Success）。

## 4. 可访问性

- 键盘：原生 `<button>`；Enter/Space 触发；图标按钮必有 `aria-label`。
- 焦点：focus-visible 全局 stroke-focus-ring 环（base.css 先例保留为规范）。
- ARIA：loading 态 `aria-busy=true`；禁用 `aria-disabled`（用 disabled 属性）。
- 对比度：on-action × primary（5.82/8.46）、on-danger × danger（8.53/6.34）、secondary 描边识别边界级（border-strong ≥3:1 双过）——均入 contrast_pairs。
- 触达：标准型 icon-size-touch-min 高（超 WCAG 2.5.8 的 24px）；紧凑型 ≥24px + 间距补足。

## 5. 主题表现

不敏感（型色全双值）；暗色 secondary 底为 surface-raised 深值 + 亮描边。

## 6. 用法与反例

✅ 表单提交 primary + 返回列表 secondary；行删除 icon-danger + 确认对话。
❌ 两个 primary 并排（主次不分）。
❌ disabled 态用纯灰 #999 之类未定义色导致对比不足。
❌ link 型用于执行写操作（链接语义 = 导航）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 映射 | 全局类 | `Button` + MFD 色（primary 槽） | `Button` + `.tint(.primaryAction)` |
| compact | 表格行内 | `TextButton` | `.buttonStyle(.borderless)` |
| loading | 旋转符 | `CircularProgressIndicator` 内嵌 | `ProgressView` 内嵌 |

## 8. 变更记录

- v1.0：统一现状 5 处散落按钮类（审计 S-2 部分归一）；新增 loading 与防误触键盘规则。
