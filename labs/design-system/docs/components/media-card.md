# 媒体卡片（media-card）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：现有提取（MediaArtwork ▣/□ 本地封面登记）+ 媒体墙/详情扩展
> 海报卡与海报墙：媒体内容的展示单元——本组件是"内容不是界面"原则（dark-media-principles）的主要执行者。🔆

## 1. 解剖

```text
┌ media-card ──────────────────────────────────────┐
│ ┌ 海报区 ───────────────────────────┐            │
│ │ [占位底]（加载前 media-placeholder）│            │
│ │ [海报图] 原图呈现，object-fit: cover│            │
│ │ [角标]（可选）badge-bg + badge-text │            │
│ │ [底部渐变+标题]（可叠字时才用 scrim-bottom）│     │
│ └───────────────────────────────────┘            │
│ [标题] 标题（font-size-sm，semibold，最多 2 行省略）│
│ [元信息行] 年份 · 类型 · 状态徽章（text-secondary，xs）│
└──────────────────────────────────────────────────┘
```

- 海报宽高比 2:3（电影/剧集通用）；墙格：最小卡宽 8.5rem，`repeat(auto-fill, minmax(…))` 流体；卡间距 `space-3`；卡圆角 `radius-lg`（海报区同角裁切）。
- 占位：`color-media-placeholder` 底 + 媒体类型首字或 ▢ 图形；加载失败保留占位 + 行内"封面缺失"提示（不显示裂图）。
- 本地封面登记状态（MediaArtwork 先例）：`▣` 已登记 / `□` 未登记（形状双编码，title 说明），不做第三方图片拼 URL。

## 2. 状态 × 变体矩阵

变体：`wall`（墙格）/ `detail-hero`（详情头图，宽幅 16:9 背景可选）/ `thumb`（行内缩略，2:3 mini）。

| 状态 | 表现 |
|---|---|
| 占位 | media-placeholder + 类型图形 |
| 加载中 | 占位底 + ◌（不闪跳动：容器固定宽高比） |
| 已加载 | 原图 cover |
| 加载失败 | 占位 + "封面不可用" |
| hover（可点卡） | 卡抬升 shadow-2 + 标题变 link 色；**图片不变暗不变亮** |
| 角标 | 类型角标（"剧集"/"通用"）或状态角标（"待确认"不进媒体页——见反例） |

双主题：容器/占位/角标 token 双值；**海报图两主题零处理**（🔆 §5）。

## 3. 行为与交互

- 整卡点击跳详情；键盘可达（可聚焦卡块）。
- 墙格滚动：服务端分页 + 触底加载（Loading 尾行）；返回保留滚动位置。
- 无限列表上限：一次会话最多自动加载 5 页，之后出现"继续加载"显式按钮（防失控拉取）。
- 详情 hero 背景图上叠标题时，标题区必须落在 scrim-bottom 最深处（遮罩 ≥0.7 不透明度区）。

## 4. 可访问性

- 键盘：卡 tabindex=0 + Enter 跳转；卡片内无嵌套交互（角标非交互）。
- 焦点：focus-visible 环在卡外框（radius-lg 同角）。
- ARIA：卡为链接语义（`<a>` 包裹或 role=link + aria-label="《标题》(年份) 详情"）；图片 alt=标题；占位 `aria-hidden`。
- 对比度：叠字标题（badge-text 白字）× 遮罩最深处 ≥4.5 由遮罩不透明度保证（dark-media-principles §2.4，布局约束非 token 对）；角标 badge-text × badge-bg 恒定深底白字不依赖主题。
- 双编码：登记状态 ▣/□ + title；角标文字本身即语义。

## 5. 主题表现（🔆 本组件为暗色媒体原则的核心执行者）

1. 海报/剧照**不二次调暗**（禁止 overlay/brightness filter，dark-media-principles §1）。
2. 暗色层级：卡底 surface + `stroke-hair` 宽 border-default 描边替代阴影可见性（§2.2）；hover 用 shadow-2 深黑阴影。
3. 占位/控制面变暗（placeholder 深值、角标深底）。
4. scrim-bottom 仅用于承载文字，纯图卡不加遮罩。

## 6. 用法与反例

✅ 暗色媒体墙：深色页面 + 原色海报 + 描边分层——海报是页面最亮元素，视觉焦点自然落在内容上。
✅ 通用视频无海报：类型占位符 + 标题可读。
❌ 暗色下给整个墙加 rgba(0,0,0,.3) 罩（违反 §1）。
❌ 待确认项目出现在媒体墙（媒体页只展示正式记录，web-experience §6）。
❌ hover 时海报放大/变亮（内容不被主题化处理，仅容器反馈）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 加载 | 原生 img + 占位 | Coil（无 colorFilter） | AsyncImage（无 overlay 暗化） |
| 墙格 | CSS grid auto-fill | LazyVerticalGrid | LazyVGrid |
| 触达 | 卡整体 | 卡整体（42dp+） | 卡整体 |

## 8. 变更记录

- v1.0：从 MediaArtwork 登记态扩展为完整海报卡/墙规范；确立"图片零主题处理"规则。
