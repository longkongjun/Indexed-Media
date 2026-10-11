# Token 映射 · Tailwind CSS 4（apps/web 前瞻/可选路径）

> 前提说明：apps/web 当前**未使用 Tailwind**（手写 CSS custom properties 方案，见审计 D-0002）。本表供两个场景：① 未来 apps/web 迁移 Tailwind 4 时的 `@theme` 映射；② 任何以 Tailwind 4 为基础的新端。现状直接采纳走 CSS 变量路径（见 adoption-roadmap）。

## 映射原则

Tailwind 4 的 `@theme` 直接接受 CSS 变量定义，语义 token 以 `--mfd-*` 引用而非复制数值——保持 tokens.css 唯一生成物：

```css
/* apps/web 的主题层（tailwind 入口） */
@import "tailwindcss";
@import "./tokens.css"; /* labs/design-system 生成产物副本 */

@theme inline {
  /* 色彩：Tailwind 工具类 → MFD 语义 token */
  --color-canvas: var(--mfd-color-bg-canvas);
  --color-surface: var(--mfd-color-bg-surface);
  --color-surface-raised: var(--mfd-color-bg-surface-raised);
  --color-sunken: var(--mfd-color-bg-surface-sunken);
  --color-selected: var(--mfd-color-bg-selected);
  --color-inverse: var(--mfd-color-bg-inverse);
  --color-ink: var(--mfd-color-text-primary);
  --color-secondary: var(--mfd-color-text-secondary);
  --color-muted: var(--mfd-color-text-muted);
  --color-link: var(--mfd-color-text-link);
  --color-primary: var(--mfd-color-action-primary);
  --color-primary-hover: var(--mfd-color-action-primary-hover);
  --color-danger-solid: var(--mfd-color-action-danger);
  --color-success: var(--mfd-color-state-success);
  --color-success-bg: var(--mfd-color-state-success-bg);
  --color-warning: var(--mfd-color-state-warning);
  --color-warning-bg: var(--mfd-color-state-warning-bg);
  --color-danger: var(--mfd-color-state-danger);
  --color-danger-bg: var(--mfd-color-state-danger-bg);
  --color-info: var(--mfd-color-state-info);
  --color-info-bg: var(--mfd-color-state-info-bg);
  --color-line: var(--mfd-color-border-default);
  --color-line-strong: var(--mfd-color-border-strong);

  /* 尺寸类：命名空间对齐 */
  --spacing-*: initial; /* 关闭默认间距，强制 4pt 阶 */
  --radius-sm: var(--mfd-radius-sm);
  --radius-md: var(--mfd-radius-md);
  --radius-lg: var(--mfd-radius-lg);
  --radius-xl: var(--mfd-radius-xl);
  --text-xs: var(--mfd-font-size-xs);
  --text-sm: var(--mfd-font-size-sm);
  --text-base: var(--mfd-font-size-base);
  --text-lg: var(--mfd-font-size-lg);
  --text-xl: var(--mfd-font-size-xl);
  --text-2xl: var(--mfd-font-size-2xl);
  --font-ui: var(--mfd-font-family-ui);
  --font-mono: var(--mfd-font-family-mono);

  /* 断点 */
  --breakpoint-sm: 48rem;
  --breakpoint-md: 64rem;
  --breakpoint-lg: 90rem;
}
```

## 主题切换机制

tokens.css 已内置三态机制：`[data-theme="dark"]` 覆盖 + 无属性时 `prefers-color-scheme` 跟随系统。Tailwind 侧**不要**用 `dark:` 变体复制颜色逻辑——组件只消费语义变量，主题由 tokens.css 统一切换。若需 JS 判定当前主题，读 `document.documentElement.dataset.theme` 或 `matchMedia('(prefers-color-scheme: dark)')`（见交互模式文档"主题切换"）。

## 对应关系速查

| MFD 语义 | Tailwind 4 工具类示例 |
|---|---|
| `color-bg-surface` 卡片 | `bg-surface` |
| `color-text-primary` 正文 | `text-ink` |
| `color-state-warning` 警示文字 | `text-warning` |
| `color-action-primary` 主按钮 | `bg-primary text-white dark:text-[--mfd-color-text-on-action]`——禁止：按钮文字色必须用 `color-text-on-action`（`text-(--mfd-color-text-on-action)`） |
| `space-4` 卡片内边距 | `p-4`（4pt 阶下 `p-4` = 1rem） |
| `radius-lg` 卡片 | `rounded-lg` |

> 反例：任何组件类中出现 `bg-[#08744b]`、`text-emerald-600` 等字面色/调色板色，均视为未采纳规范。
