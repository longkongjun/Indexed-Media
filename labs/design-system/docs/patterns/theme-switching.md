# 交互模式 · 主题切换（theme-switching）

> 版本：v1.0 ｜ 基准：tokens.css 三态机制（`:root` 浅色 / `[data-theme=dark]` / `prefers-color-scheme` 回退）+ 设计原则 5"主题是环境不是皮肤"。

## 1. 三态模型

| 态 | data-theme | 行为 |
|---|---|---|
| 跟随系统（默认） | 无属性 | 由 `prefers-color-scheme` 媒体查询驱动；系统切换时实时跟随 |
| 手动浅色 | `light` | 恒浅色 |
| 手动暗色 | `dark` | 恒暗色 |

初始加载防闪白（FOUC）：启动内联脚本在首帧前读本地存储设 `data-theme`（存在则设，不存在留给媒体查询）；脚本无框架依赖（apps/web 现状零依赖兼容）。

## 2. 设置入口与控件

- 位置：**设置 → 系统 → 外观**（管理面全局偏好，非每设备埋深处）；概览不做快捷切换（避免"环境设置"被误认为状态指示）。
- 控件：三选一分段控件（radio group 语义）：`跟随系统 / 浅色 / 暗色`，当前态 `aria-checked`；快捷循环按钮（太阳/月亮图标）**不提供**（循环控件无法呈现三态，且误触率高）。

## 3. 选择记忆与跨端同步

- 记忆：本地存储（Web：`localStorage` 键 `mfd-theme`，值 `system|light|dark`）。
- 跨端同步策略（分阶段）：**v1 不同步**（各端各自记忆——主题是环境偏好，跨端强同步反而违背"跟随当地环境"直觉）；未来若账号级偏好同步，仅同步"手动浅/暗"选择，`跟随系统` 永远按当地设备解析。写入采纳路线图供消费方评估。
- 移动端：跟随系统 = 系统深色模式（Compose `isSystemInDarkTheme` / SwiftUI trait）；应用内切换同三态模型。

## 4. 切换过渡

- 过渡：容器层交叉淡入 `motion-duration-slow`（400ms）+ `motion-easing-in-out`；仅过渡颜色类属性（`transition-property: color, background-color, border-color, fill, stroke`），布局属性不过渡。
- `prefers-reduced-motion: reduce`：立即切换（0ms）。
- 图片零处理：切换瞬间海报/剧照不变（dark-media-principles §1）——视觉上仅"环境"变化。

## 5. 暗色下的状态与内容表现（汇总锚点）

| 对象 | 暗色表现 | 出处 |
|---|---|---|
| 六态容器 | 深 tint 底；Offline 横幅 surface 底 | state-container §5 |
| 会话消息 | selected 深绿 tint 区分角色 | session-message §5 |
| 媒体墙 | 原图 + 描边分层 | media-card §5 |
| 图表 | 亮系列色 + 深网格 | dark-media-principles §5 |
| 阴影 | 深黑阴影 + 描边补偿 | shadow token 暗色值 |

## 6. 验收锚点

1. 三态切换全部组件 demo 双主题可视核查（风格指南站 D4）；
2. 刷新/深链/返回不闪错主题（含 SSR 禁忌：无服务端渲染，无此风险点但仍验）；
3. 系统主题变化时"跟随系统"态实时切换且无布局跳动；
4. reduced-motion 下无过渡动画。
