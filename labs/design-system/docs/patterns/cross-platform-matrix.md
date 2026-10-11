# 跨端适配矩阵（cross-platform-matrix）

> 版本：v1.0 ｜ 基准：tech-stack §6 跨端统一契约 + 组件规格平台差异列的汇总视图。
> web → Compose → SwiftUI 的组件与 token 映射原则：**同一语义，各端原生实现**。

## 1. 组件映射总表

| MFD 组件 | Web | Compose（M3） | SwiftUI | 移动端等价性 |
|---|---|---|---|---|
| button | 全局类 button | Button（MFD 色） | Button + tint | 全量 |
| form | form-card + 原生控件 | OutlinedTextField 等 | TextField 等 | 全量（写操作视权限） |
| table-list | table/list + CursorPager | LazyColumn（list 形态） | List | list 形态等价；表格降级 |
| media-card | CSS grid 墙 | LazyVerticalGrid + Coil | LazyVGrid + AsyncImage | 全量（消费面核心） |
| status-badge | span 全局类 | AssistChip/自绘 | Label | 全量（词表共享） |
| task-timeline | ol 手风琴 | LazyColumn + Disclosure | VStack + DisclosureGroup | 只读全量；恢复动作视权限 |
| plan-preview | full + summary | summary 只读 | summary 只读 | **审批仅 Web** |
| approval-banner | 页内横幅 | 不渲染（跳 Web） | 不渲染 | 仅 Web |
| session-message | 侧栏抽屉 | 第二批评估 | 第二批评估 | — |
| notification | 中心页 | 只读中心页 | 只读中心页 | 推送第二批 |
| navigation | 侧栏/底部导航 | NavigationSuiteScaffold | TabView/sidebar | 全量 |
| state-container | AsyncState 扩展 | sealed UiState | enum + overlay | 全量 |

## 2. token 消费原则

1. **语义对齐**：各端建自己的常量层（CSS 变量 / MfdColor 对象 / Color 扩展），值一次性固化自 tokens.json（版本注释锚定）；**禁止各端自行调值**。
2. **映射表为准**：Compose/SwiftUI 的槽位映射见 docs/mappings/ 两表；映射表未覆盖的 token 扩展槽承载。
3. **不变量**：状态色语义（成功绿/待处理琥珀/失败红/进行中蓝）、六态语义、双编码（形状+文字）、触达 44、等宽用于技术标识——四端一致（这是"跨端统一契约"的 UI 面）。

## 3. 行为等价验收（供 m5-client 一致性矩阵消费）

- 同一 fixtures 下：媒体墙渲染项数/排序一致；状态徽章词表一致；错误文案模板一致（对象+原因+下一步）；分页语义一致（游标 + 计数）。
- 差异允许项：导航形态、列表 vs 表格形态、写操作入口（移动端只读）、审批入口（仅 Web）。

## 4. 窗口尺寸分级与平台形态（M3 window size classes 对齐）

- **compact**（< 600dp / 48rem）：底部导航四项、单列、表格降级列表；触达 44dp。
- **medium**（600–840dp / 48–64rem）：侧栏图标列、单列为主。
- **expanded**（> 840dp / 64rem）：固定侧栏、双列可用（90rem 上限）。
- 尺寸分级判断窗口而非设备（M3 原则）——折叠屏/分屏自动落位。
- 平台可视化对照：风格指南站"平台 · 手机/TV"视图（真实渲染，非截图）。

## 5. 皮肤维度（v2.0 起）

三端共享同一皮肤体系（docs/skins.md）：Web 走 data-skin 属性；Compose 每皮肤一组 MfdColor 常量（或接 M3 dynamic color，tone 同构）；SwiftUI 经 Environment 注入 MfdTheme。皮肤 × 主题全组合过 AA 是发布门禁。

## 6. TV 适配原则（M6 预置，不做专门设计）

- 只做媒体墙/详情/状态/外部播放跳转（tech-stack §5）；遥控器焦点 = 键盘焦点规范的九宫格扩展（方向键导航、Enter 确认、返回 Esc 语义）。
- token 直接消费 TV 端暗色偏好（观影环境默认暗色合理）；触达规范以焦点块尺寸替代（≥ focus 块 48dp）。
- 管理面（审批/恢复/配置）不出现在 TV。
