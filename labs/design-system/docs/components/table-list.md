# 表格与列表（table-list）

> 版本：v1.0 ｜ 状态：定稿 ｜ 来源：现有提取（activity-table / notification-list / CursorPager / .view-tabs 筛选）
> 管理数据的三种密度形态：数据表 / 列表行 / 迷你行 + 游标分页 + 筛选工具栏；"只读事实流"页面的标准骨架。

## 1. 解剖

```text
┌ list-page ────────────────────────────────────────────────┐
│ [页头] h1 + 说明段（页面职责一句话）                          │
│ [工具栏] 筛选控件（tabs/checkbox/select）+ 计数              │
│ [state-container]（四态包裹）                                │
│ [数据表] thead（medium，底 bg-selected）+ tbody 行           │
│   或 [列表] ul > li 行卡片（底 surface + border-subtle 分隔） │
│ [CursorPager] 上一页/下一页（游标不透明透传）                 │
└────────────────────────────────────────────────────────────┘
```

- 表格：`font-size-sm`；th 底 `color-bg-selected`、字 `color-text-secondary` medium；行分隔 `color-border-subtle`（装饰级）；单元格纵 `space-2` 横 `space-3`。
- 列表行：整行可点（跳详情）时为可聚焦块，hover 底 `color-bg-hover`，行内次操作按钮 stopPropagation。
- 等宽单元格（ID/路径/错误码）用 `font-family-mono`；ID 截断显示 `前 8 位…` + title 全量（现状先例）。
- 虚拟化：>100 行启用虚拟滚动（TanStack Virtual 目标态；现状全量渲染）。

## 2. 状态 × 变体矩阵

变体：`table`（多列事实）/ `list`（行卡片）/ `rows`（迷你行，如 AiSessionPanel 事件流）。

| 状态 | 表现 |
|---|---|
| 默认 | 如上 |
| hover（可点行） | bg-hover |
| selected（当前项） | bg-selected + 左条纹 border-strong |
| 加载更多 | CursorPager 内 ◌ + 禁用 |
| 空态/错误/离线 | state-container 包裹（表格不渲染半空壳） |

筛选 tabs（.view-tabs 先例）：激活底 `color-bg-selected` + 计数胶囊（`radius-full`，底 selected、字 secondary）。

双主题：全部 token 双值成立。

## 3. 行为与交互

- 游标分页（CursorPager）：上一页/下一页 + 位置提示（"已显示 N 项"）；游标对用户不透明；返回保留筛选与滚动位置（web-experience 原则 4）。
- 筛选切换即重新加载（Loading + stale 列表共存）；筛选状态写入 URL query（可分享/可回退）。
- 行点击整行跳转；行内操作按钮（已读/取消等）独立点击不冒泡。
- 宽表响应式：< sm 断点切列表形态（现状 `[data-wide-results]` 先例规范化为：table 变体必须声明移动端 list 降级）。

## 4. 可访问性

- 键盘：可点行 tabindex=0 + Enter 跳转；行内按钮正常 Tab；表格无行级键盘陷阱。
- 焦点：跳转返回后焦点回原行（列表上下文恢复）。
- ARIA：表格 `caption`（sr-only 先例保留）；筛选 tabs `role=tablist`；加载更多按钮 `aria-describedby` 位置提示。
- 对比度：表头/正文文字对全过；行分隔装饰级豁免（D-0006）；hover 底为叠加层（半透明，不承担文字对比——文字对 surface 已过）。
- 双编码：状态列用 status-badge；未读等强调不只靠色（条纹+文字"未读"）。

## 5. 主题表现

不敏感；暗色表头 selected 为深绿 tint、行分隔深值——视觉密度与浅色一致。

## 6. 用法与反例

✅ capability-events 页：表 + sr-only caption + 错误码 mono + 任务链接。
✅ 通知中心：list 变体 + 未读条纹 + 行内"标记已读"。
❌ 空数据时渲染空表格骨架（应转 state-container Empty）。
❌ 错误时保留旧数据且无任何提示（必须有 Offline/Error 态标记 stale）。

## 7. 平台差异

| 维度 | Web | Compose（M3） | SwiftUI |
|---|---|---|---|
| 表格 | table 或网格 div | LazyColumn（移动端不鼓励多列表格，转 list 卡片） | List（同左） |
| 虚拟化 | TanStack Virtual | LazyColumn 内建 | List 内建 |
| 分页 | CursorPager | 同语义（下一页按钮 + 计数） | 同 |
| 移动降级 | table→list | 直接 list | 直接 list |

## 8. 变更记录

- v1.0：归一 activity-table/notification-list/CursorPager 三先例为统一骨架；新增筛选入 URL、返回恢复、虚拟化阈值规则。
