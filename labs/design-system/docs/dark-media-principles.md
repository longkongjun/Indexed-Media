# 暗色主题下的媒体内容处理原则

> 适用：海报墙、媒体详情、会话中的缩略引用、图表等图像密集场景。原则同样约束浅色主题，但冲突集中在暗色。

## 1. 核心原则：内容不是界面

海报、剧照、缩略图是**用户内容**，不是主题装饰。暗色主题只改变"容器与控制面"（页面底、卡片底、边框、文字、图标、按钮），**不二次调暗图片本体**：

- 禁止对 `<img>`/AsyncImage/Coil 图加全局暗化 overlay、brightness filter 或半透明黑罩。
- 禁止用 `filter: brightness(.8)` 之类的"氛围处理"追求页面统一感。
- 占位图（加载前）用 `color-media-placeholder`（暗色为深绿黑），加载完成即原图呈现。

理由（决策 D-0005）：媒体库的暗色场景诉求是"看片环境不刺眼"——刺眼来自大面积高亮**界面**底色，而非海报；压暗海报破坏内容还原与色彩判断（管理员要核对版本/画质）。

## 2. 容器层的主题融合

图片与暗色环境的融合靠容器，不靠改图：

1. **卡片底**：媒体卡在暗色用 `color-bg-surface`（#121b16），海报区域与卡片同圆角（radius-lg）。
2. **边框替代阴影**：暗色下阴影几乎不可见，媒体卡用 `1px color-border-default` 描边 + shadow-1（深黑阴影）保层级。
3. **控制面变暗**：图片上的常驻控件（播放按钮、角标、进度条）用 `color-media-badge-bg`（≈85% 黑）底 + 白字，两主题一致——控件需要压住任意图片内容，本来就该用中性深底。
4. **底部渐变遮罩仅用于承载文字**：卡片上叠标题时用 `color-media-scrim-bottom`（底部 ≥0.7 不透明度渐变），并保证文字落在渐变最深处；纯图片卡不加遮罩。

## 3. 六态在暗色下的表现

| 态 | 暗色处理 |
|---|---|
| Loading | 骨架屏底 `color-bg-surface-sunken`，脉冲动画保留（reduced-motion 时静态） |
| Empty | 图标 `color-text-muted`、文案 `color-text-secondary`，占位插画不用大面积浅色块 |
| Error | `color-state-danger-bg`（深红棕底）+ danger 前景 + 图标，比浅色更克制（暗色下红色少发光） |
| Offline | warning-border 描边横幅 + warning 前景，底用 `color-bg-surface`（非 warning-bg，避免暗色大面积黄底刺眼） |
| Conflict / Partial | 徽章/横幅走 state-warning 系；Partial 列表"已完成/未完成"分组线用 border-subtle |
| Success | success 前景 + success-bg 底；文件操作结果确认不自动消失 |

## 4. 会话消息与代码/路径

- 会话消息（AI 会话）：消息底 `color-bg-surface`，系统/AI 与用户消息靠对齐+徽章区分，不靠底色深浅对撞；暗色下等宽路径片段用 `color-bg-surface-sunken` 井底 + 白绿文字。
- 代码/路径块：等宽字体 + sunken 底 + 允许安全换行与复制；暗色不做"语法高亮换成亮色主题色板"之外的定制。

## 5. 图表

- 暗色用 chart-1..5 暗色值（亮色系列色）+ `chart-grid`（深线）；网格线装饰性，不要求 3:1。
- 图表底永远 `color-bg-surface`，不为图表单独做透明底混合页面。

## 6. 验证方式

- D4 风格指南站媒体墙 demo 内置主题切换，逐 demo 双主题可视核查；
- 对比度自动检查覆盖文字/控件对（见 tokens.json contrast_pairs），图片上文字的达标由遮罩最深处不透明度保证（布局约束，非 token 计算）。
