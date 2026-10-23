# @mediaflow/ui-vue

MediaFlow Web 正式基础库的最小切片：仅包含 C01 Action 按钮（`MfButton`）。设计契约来自 `references/Indexed-wiki/design/ui-system/`（02 共同契约、03 主题与数值来源、10 Web 组件、13 正式库采纳契约，B1-1）；本包是 13 所述"正式 Vue 基础库"的首个可评审切片，不代表完整 Web 基础库。

## 公共 API

```ts
import { MfButton, type MfButtonProps, type MfButtonType, type MfButtonVariant } from "@mediaflow/ui-vue";
import "@mediaflow/ui-vue/style.css";
```

| prop | 类型 | 默认 | 说明 |
|---|---|---|---|
| `variant` | `"default" \| "primary" \| "danger" \| "icon"` | `"default"` | `icon` 为仅图标方形按钮，标签视觉隐藏但保留可访问名称 |
| `type` | `"button" \| "submit" \| "reset"` | `"button"` | 默认不在表单中意外提交；表单提交用途显式传 `submit` |
| `disabled` | `boolean` | `false` | 原生 `disabled`；不响应指针与键盘 |
| `busy` | `boolean` | `false` | `aria-busy` + 忙碌指示；保留焦点与名称，拦截再次激活（含 submit 型原生提交） |

- 事件：`activate`（无负载；disabled/busy 不发出）。
- 插槽：`default`（标签，始终是可访问名称来源）、`icon`（前置图标，仅装饰、`aria-hidden`）。
- 未声明属性（`aria-label`、`title`、`form`、`class` 等）透传到原生 `button`。
- 组件是原生 `<button>`：Tab/Enter/Space、焦点与表单语义全部走浏览器原生行为，不设 `tabindex`。
- `variant="icon"` 缺少可访问名称时开发模式 `console.warn` 一次；生产不告警。
- 尺寸：默认最小 44×44 CSS px（min-height/min-width，非固定高度），16/1.5、radius-md、内边距 水平16/垂直8；字号放大按内容撑高，长标签换行（`overflow-wrap: anywhere`）。

## 消费方式

```vue
<script setup lang="ts">
import { MfButton } from "@mediaflow/ui-vue";
import "@mediaflow/ui-vue/style.css";
</script>
<template>
  <MfButton variant="primary" type="submit" :busy="saving" @activate="save">保存</MfButton>
  <MfButton variant="icon" aria-label="刷新" @activate="refresh">
    <template #icon><svg><!-- ... --></svg></template>
    刷新
  </MfButton>
</template>
```

依赖：`vue` 为 peer 依赖（^3.5），构建产物不内联 Vue 运行时。本包不依赖 Core API、KMP SDK、Pinia、路由、业务实体或真实配置，不联网，不含面向用户的固化文案。包当前为 `private`，未发布。

## 主题与 token 来源

`src/styles/theme.css` 提供按钮消费的 `--mfd-*` 语义 token 子集（evergreen 浅/暗），值逐项来自冻结值源 `labs/design-system/tokens/tokens.json` v2.0.0（与 `apps/web/src/styles/tokens.css` 同源），由 `tests/package-contract.test.ts` 对账。主题结构与生成表一致：`[data-theme="light|dark"]` 显式优先（包含子树），无偏好时跟随系统；皮肤（aurora/sepia）由宿主完整 token 表提供，宿主同名变量可覆盖本子集（值同源，顺序无关）。组件样式选择器全部以 `.mf-button` 限定，不选中宿主其他 `button`。

已记录的规范映射决定（规范未定或 lab 旧文档冲突处，以 wiki B1-1 为准）：图标尺寸取 `icon-size-md`（token desc"按钮与导航图标"）；danger pressed 无 active token，与 hover 同值；普通按钮 pressed 以 `border-strong` 内阴影加深边框；hover 仅在 `(hover: hover)` 下出现，普通型 hover 用 `bg-hover` 叠层保留底色。

## 构建与验证

```bash
pnpm typecheck        # vue-tsc --noEmit
pnpm test             # 单元 + 包契约测试（不含消费者）
pnpm build            # vite lib 构建 + vue-tsc 声明 → dist/index.js, dist/index.d.ts, dist/style.css
pnpm test:consumer    # 从构建产物公开入口消费的最小无业务消费者测试
pnpm verify           # 以上全部按序执行
```

验证边界：jsdom 单元测试覆盖 DOM 契约与事件；几何（44×44、200% 文字、320px 长标签换行）、focus-visible 焦点环、暗色主题与真实键盘激活由浏览器验证记录（见工作流状态 `workflow-state/mediaflow/worktrees/ui-web-action-slice/`）。主任务在桌面Chrome补验：Enter/Space激活一次、busy保焦点/防重复、系统暗色回退、显式浅色子树、reduce-motion静态忙碌指示和200%换行。真实读屏、RTL、触屏及其他浏览器引擎仍待验。
