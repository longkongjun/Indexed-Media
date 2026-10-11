# 导航（navigation）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：现有提取（AppShell 侧栏/账户面板）+ 响应式三档（web-experience §8）
> 应用骨架导航：宽屏固定侧栏 → 中宽图标侧栏 → 移动底部导航；含页签、角标计数、全局 Offline 横幅位。

## 1. 解剖

```text
宽屏（≥ md 断点）                    移动（< sm 断点）
┌─sidebar(bg-inverse)─┬─main────┐   ┌─main────────────┐
│ [产品名]             │ 页面内容 │   │ 页面内容          │
│ [导航项 × N]         │         │   ├─bottom-nav──────┤
│  ├ 概览/任务/媒体/    │         │   │ 概览 任务 媒体 更多│
│  │ 资源库/设置        │         │   └─────────────────┘
│  └ 角标计数(任务)     │         │   "更多"= 资源库+设置
│ [AI 会话面板挂载位]   │         │   （抽屉/子页）
│ [账户区+账户面板]     │         │
└─────────────────────┴─────────┘
```

- 侧栏底 `color-bg-inverse`、文字 `color-text-inverse`、次级 `color-text-inverse-muted`、hover 底 `color-bg-inverse-hover`、激活项底 inverse-hover + 左条纹 border-focus；分隔线 border-inverse。
- 底部导航：底 surface + border-default 上边线、4 项等分、激活项 accent 图标+文字、图标 icon-size-md、高 touch-min。
- 中宽档（sm–md）：侧栏收窄为图标列（宽 `space-12`），产品名转首字标，导航项 title 提示 + aria-label 全量。
- 导航角标：`radius-full` 胶囊，底 `color-action-danger`（仅"需要管理员介入"计数，口径见 notification）字白（on-danger 对已验）。

## 2. 状态 × 变体矩阵

| 元素 | 默认 | hover | 激活 | 禁用 |
|---|---|---|---|---|
| 侧栏项 | inverse 文字 | inverse-hover 底 | 左条纹 + inverse-hover 底 + semibold | opacity .55 + aria-disabled（如未配置时隐藏而非禁用） |
| 底部导航项 | secondary 文字 | bg-hover | accent 图标 + semibold | 同上 |
| 角标 | — | — | 计数值 | 0 时隐藏（不显示 0） |

全局 Offline 横幅位：main 顶部通栏（state-container Offline 态的页面级实例，warning 描边 + surface 底），出现时禁用全部需服务端确认的操作（按钮 disabled + 说明），导航仍可浏览缓存视图。

双主题：全部 token 双值；暗色侧栏 inverse 暗值为最深层层级。

## 3. 行为与交互

- 路由与导航一一对应；当前路由项 aria-current="page"。
- 中宽图标模式与宽屏模式**同一 DOM 顺序**（仅样式变化），保证读屏顺序一致。
- 移动"更多"打开抽屉（z-modal + scrim）；Esc/点 scrim 关闭；焦点陷阱内含资源库与设置全部入口。
- 返回保留：导航切换不重置列表页筛选与滚动（table-list 配合）。
- AI 会话面板（session-message 容器）挂载于侧栏底部（现状先例）；移动端在"更多"抽屉内。

## 4. 可访问性

- 键盘：导航项原生链接 Tab 序；底部导航同；抽屉内 Tab 循环 + Esc 关闭。
- 焦点：路由切换后焦点移至主内容区开头（`<main tabindex=-1>` 聚焦）；抽屉打开焦点入抽屉、关闭还焦点给触发器。
- ARIA：`nav aria-label="主导航"`；激活 `aria-current="page"`；角标 `aria-label="N 项待处理"`；抽屉 `role=dialog aria-modal`。
- 对比度：侧栏文字对（inverse/inverse-muted × bg-inverse）双过 4.5（contrast_pairs）；角标 on-danger × danger 全过；激活左条纹 border-focus 为识别边界级。
- 触达：全部导航项高 touch-min；底部导航图标+文字双示。

## 5. 主题表现

不敏感（侧栏两主题均为深底——浅色主题侧栏本就是品牌深绿黑，暗色主题全深一体的层级差靠 surface 阶梯）。

## 6. 用法与反例

✅ 任务角标只统计待确认+冲突+失败（需介入项），完成态不计数。
✅ Offline 横幅出现时"提交/审批"类按钮禁用并说明原因，浏览不受阻。
❌ 移动端塞下全部五个一级导航（四项 + 更多）。
❌ 角标显示总数而非待处理数（制造焦虑，违反"计数即行动指引"）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 骨架 | AppShell | `NavigationSuiteScaffold`（M3 Adaptive：rail/bottom 自适应） | `TabView` + sidebar（iPad） |
| 角标 | 胶囊 | `BadgedBox` | `.badge` |
| 抽屉 | dialog 语义 | `ModalDrawerSheet` | `.sheet` |

## 8. 变更记录

- v1.0：规格化 AppShell + 响应式三档（断点与 token 对齐：sm 48rem / md 64rem）；明确 Offline 通栏与角标口径。
