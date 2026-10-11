# 输入与表单（form）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：现有提取（.form-card / .check-field / ErrorSummary.vue / apiErrors）
> 文本/选择/复选/单选 + 字段标签-帮助-错误结构 + 表单卡片容器；含只读等宽展示（路径/规则）。

## 1. 解剖

```text
┌ form-card（bg-surface, radius-lg, border-default, padding space-4）─┐
│ field × N：[标签 label]（medium，必填标 *）                          │
│   [控件] input/select/checkbox/radio                                │
│   [帮助文字]（text-secondary，font-size-xs）                        │
│   [字段错误] ⚠ 文案（danger 前景 + danger-bg 行，role=alert 关联）    │
│ [ErrorSummary]（提交级错误汇总，可聚焦，锚链接到字段）                │
│ [操作区] 提交(primary) + 取消(secondary)                            │
└────────────────────────────────────────────────────────────────────┘
```

- input/select：底 `color-bg-surface`、描边 `color-border-strong`（识别边界级）、圆角 `radius-md`、高 touch-min、focus 时描边 `color-border-focus` + 焦点环。
- checkbox/radio（.check-field 先例）：横向行，标签可点击，选中色 `color-action-primary`，勾选符形状双编码。
- 只读等宽块：`font-family-mono` + `color-bg-surface-sunken` + 允许安全换行 + 复制按钮（secret 类一次性值走 SecretOncePanel 模式：展示一次 + 复制 + 确认已存）。
- 表单网格：label 上置两列（宽屏）/ 单列（< md）；max-width 36rem（现状先例保留为表单档）。

## 2. 状态 × 变体矩阵

| 控件状态 | 表现 |
|---|---|
| 默认 | border-strong 描边 |
| focus | border-focus 描边 + stroke-focus-ring 焦点环 |
| 错误 | border-danger 级描边（state-danger-border）+ 字段错误行 |
| 禁用/只读 | 底 sunken + text-muted；只读等宽块仍可复制 |
| 加载中选项（select 异步） | 控件内 ◌ + 禁用 |

409 冲突保护（现状先例升级为规范）：提交遇 `request.conflict/resource.conflict` 时**保留全部草稿**，表单顶部 ErrorSummary 说明冲突对象与"重新计算/改名重试"路径，不清空不跳转。

双主题：全部 token 双值；focus 描边双过 3:1（contrast_pairs 非文本组）。

## 3. 行为与交互

- 校验时机：失焦单字段校验 + 提交全量校验；错误聚焦第一个错误字段（ErrorSummary 锚链接）。
- 密码/令牌：输入框 `type=password`；一次性值展示后不可再查（secret 语义）。
- 长输入换行规则：路径输入框单行 + 横向滚动；只读等宽块安全换行（不断词）。

## 4. 可访问性

- 键盘：控件原生 Tab 序；radio 组箭头切换；checkbox Space 切换；错误聚焦移动焦点到首错字段。
- 焦点：focus-visible 环全局基线；ErrorSummary 容器 `tabindex=-1` + 聚焦播报。
- ARIA：字段 `aria-describedby` → 帮助/错误 id；错误 `role=alert`；radio 组 `role=radiogroup`；必填用 label 文字（`*` + aria-required）。
- 对比度：标签/错误文字对全过；输入描边识别边界级（border-strong ≥3:1 双过）；错误描边 state-danger-border ≥3:1 双过。
- 触达：全部控件高 touch-min。

## 5. 主题表现

不敏感（token 双值）；暗色输入底 surface + 亮描边，只读井 sunken 深值。

## 6. 用法与反例

✅ 收件目录表单：路径输入 + 帮助文字"授权范围内的绝对路径" + 409 冲突草稿保留。
❌ 错误只描红边无文字（单靠颜色）。
❌ 只读路径用普通字体展示（必须等宽 + 可复制）。
❌ 提交失败清空表单。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 实现 | 原生控件 + 全局类 | `OutlinedTextField`/`Checkbox` | `TextField`/`Toggle` |
| 错误汇总 | ErrorSummary 组件 | `Column` 错误首置 | `Section` 错误首置 |
| 409 保护 | 草稿保留规范 | 同 | 同 |

## 8. 变更记录

- v1.0：归一现状 form-card/check-field 模式；吸收 OrganizationTargetForm 409 草稿保留为先例规范；新增只读等宽块与 secret 展示规则。
