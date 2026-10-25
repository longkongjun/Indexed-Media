# Indexed-Design 接入（Web 首批）

MediaFlow Web 分批接入 `@indexed-design/ui-vue` 设计库。本文件记录基线与依赖方式、主题衔接、组件迁移映射和 `--mfd-*` / Indexed token 的过渡策略；过程与验证证据保存在 Personal Forge 的 `adopt-indexed-design-web-media` change。

## 1. 基线与依赖

| 项 | 值 |
|---|---|
| 设计库 | `@indexed-design/ui-vue@0.15.0`（`ant-design-vue@4.2.6` + `@ant-design/icons-vue@7.0.1`） |
| 源 commit | Indexed-Design `5de0ae8`（见 `vendor/indexed-design/README.md`） |
| 接入方式 | `file:../../vendor/indexed-design/indexed-design-ui-vue-0.15.0.tgz`，tarball 校验值记入 `pnpm-lock.yaml` integrity |
| 限制 | npm 未发布；远端 `origin/main` 只有 0.14.0，**不可**用 git 依赖冒充同一基线；任何机器（含 CI）靠仓库内 tarball + 锁文件还原字节一致的包 |

`apps/web` 的 `tsconfig.json` 因 antd-vue 4.2.6 的 d.ts 与 Vue 3.5 类型声明不兼容而启用 `skipLibCheck`（Indexed-Design 仓库自身同配置）；自研代码仍受全部严格选项约束。

## 2. 主题衔接

- `main.ts` 在应用 token 前加载 `@indexed-design/ui-vue/style.css`。
- `App.vue` 全树包 `IndexedThemeProvider`（antd ConfigProvider + `.id-theme` 容器 + 主题变量），evergreen 品牌。
- 浅色/深色/跟随系统三态真源不变：`app/theme.ts` 写 root `data-theme`（system 不写属性，交给 `prefers-color-scheme`）。`app/designTheme.ts` 桥把三态解析为 provider 需要的显式 `light|dark`，监听 `mfd-theme-change` 与系统媒体查询，不新增持久化。
- **皮肤缺口**：宿主皮肤维度（aurora/sepia，写 root `data-skin`）没有 Indexed 品牌对应物（设计库仅 evergreen/cobalt）。本批不映射：选择非基线皮肤时，未迁移页面与页面级 `--mfd-*` 样式换肤，已迁移的设计组件保持 evergreen。缺口登记在设计库侧（品牌变体扩展）解决后再接，不在 Web 侧硬造品牌色。
- **弹层容器**：带弹层的设计组件（Select/DatePicker/Tooltip 等）默认挂 body，会脱离 `.id-theme` 作用域；使用时须按设计库 0.15.0 约定传 `getPopupContainer` 挂回主题区域。本批未采用带弹层组件，未引入该配置。

## 3. 组件迁移映射

原则：按语义迁移，不批量替换包名；组件级状态语义对齐后才切换；无对应能力的组件保留旧实现并注明条件。

| 旧（@mediaflow/ui-vue） | 新（@indexed-design/ui-vue） | 语义差异与迁移注意 |
|---|---|---|
| `MfButton` `type`（原生 button type，默认 `button`） | `Button` 的 `htmlType` | **不能映射到新 `type`**：新 `type` 是视觉变体（`primary`/`default`/`dashed`/`link`/`text`）。表单提交按钮必须 `html-type="submit"`，否则回退默认 `button` 不提交。 |
| `MfButton` `variant="primary"/"danger"/"icon"` | `Button` `type="primary"` / `danger` 属性 / 图标按钮 | 旧 `default` 变体 = 新默认 `type="default"`；危险 = `danger`（保留 `type` 语义）；icon 变体无直接对应，需要可访问名称的纯图标按钮暂缓迁移。 |
| `MfButton` `@activate`（disabled/busy 时不发出） | `Button` `@click` + `:loading` | 旧组件在 busy 时拦截点击；antd `loading` 同时锁交互并显示指示，宿主即时防重语义等价，但 click 事件本身仍会触发，处理函数需自带防重（设计库采用指南同要求）。 |
| `MfButton` `busy` | `Button` `loading` | 等价；antd 另提供 `aria-busy`/`aria-disabled` 显式声明（见 `web-vue.button.primary`）。 |
| `MfRequestState` | `RequestState` | props 同名（`state`/`errorMessage`/`offlineMessage`/`retryLabel`…），新增 `loadingDelay`（默认同为 300ms）。差异：空状态由 notice 条改为 `Empty` 呈现（`emptyMessage` 作为 description，`#empty` 槽移入 Empty 内），内容容器类名 `mf-request-state__content` → `id-request-state__content`；stale 旧内容 inert 行为一致。 |
| `MfContentCard` | 无 1:1；页面级组合（`.media-card`） | C14 媒体网格卡（2:3 封面+两行标题+元信息）是页面模式；antd `Card` 是带边框的内容盒卡（`web-vue.card.primary` 面向 dashboard 卡），套用会改变设计语义。已迁移页面用页面级标记+token 组合承担，样式进视图 scoped 块。 |
| `MfImage`（经 `MediaArtwork`） | 暂保留 | MediaFlow 封面是不透明本地引用，无可读 URL 时不渲染图片（`web-vue.image.primary` 面向真实 src/preview 场景）；MfImage 的占位/缺省语义当前无可替代，强行换 antd Image 会伪造图片加载态。待 Core 提供封面 URL 能力后重评。 |
| `MfField` / `MfDialog` / `MfMenu` | 已随第二批迁移/退役（见下） | `MfField`→Input（LoginView，标签→控件→错误由页面组合）；`MfDialog`→Modal（InboxListView 预检确认）；`MfMenu` 无视图消费者，随旧包保留至退役。 |

共享组件 `CursorPager`（原生按钮）与 `AppShell` 未改：前者本就是原生实现且被 9 个视图共享，后者有待其他会话的未提交修改，均留待对应页面批次。

## 3.1 第二批：应用级容器与残余表单/模态

- **`AsyncState`（应用页面状态容器，19 视图共用）**：props/`retry` 事件/`.async-state` 类钩子与 `packages/ui-contract` 状态权威不变；呈现层委托 `RequestState`（空态经 antd Empty、错误/离线通知条与重试按钮、stale 降对比 + inert）。`idle` 实现级起始态不进设计容器。文案默认值沿用应用措辞（"暂无数据"/"请求暂时失败，请重试"/"当前离线，显示上次加载的内容"）。可见差异：错误/离线通知不再带"加载失败/离线"粗体前缀；重试按钮为 antd Button（两字标签显示"重 试"）；加载中 stale 的 inert 从指示出现即刻提前到进入 stale 即生效（≤300ms 窗口差异）。
- **`MfField` → `Input`（LoginView）**：旧组件的 label→input→hint/error 结构由页面组合（`label` + `Input` + 页面级错误段落）；`name/autocomplete/required/type` 等原生属性透传到内层 input。`aria-invalid` 仅在无效时渲染（旧组件恒渲染 `"false"`），无效时同步 `status="error"` 红边。验证摘要→字段错误的关联仍在视图层。
- **`MfDialog` → `Modal`（InboxListView 预检确认）**：按 `web-vue.modal` 模式（centered、`min(38rem, calc(100vw - 2rem))`、自定义页脚）。busy 语义映射：`confirmLoading` + busy 时 `maskClosable/keyboard/closable=false` 且取消按钮 disabled（旧组件 busy 保留取消可聚焦但拦截——antd 以 disabled 表达"busy 禁关闭"，焦点保持差异已登记）。`confirmDisabled` → 确认按钮 `disabled`；初始焦点取消由宿主在打开后聚焦页脚取消按钮（antd-vue 声明式 Modal 的 `autoFocusButton` 属性存在类型但未实现，仅命令式 confirm API 消费），并使 vc-dialog 的 ESC/Tab 圈闭立即生效（圈闭依赖焦点在模态内）；关闭归还触发项经默认 `focusTriggerAfterClose`。`request-close(clean|dirty)` 收敛为 `@cancel`（本页无 dirty 分支）；`description` 以正文首段呈现，`aria-describedby` 关联不自动接线（登记为设计库模态边界）；标题用 `#title` 槽内 `h2` 保持 heading 语义（e2e 依角色名驱动）。错误回执以正文 `role="alert"` 段落保留。
- 视图层的 `@mediaflow/ui-vue` 导入至此归零（`MediaArtwork` 的 `MfImage` 按首批决策保留；`tools/ui-vue-consumer` 工作台与旧包本身不动）。

## 4. Token 过渡策略

`--mfd-*`（tokens.css，v2.0.0 evergreen 基线 + 皮肤层）与 Indexed token（`token-baseline.json`，同一 v2.0.0 基线值）目前是**两套生成物、同一数字源**：evergreen 浅/暗下两者色值、字体栈、字号一致（`#08744b`/`#12231c`/Inter 栈/1rem 基准）。

- 现阶段分工：组件视觉（antd 经 ConfigProvider 注入）与 `.id-theme` 作用域变量走 Indexed token；页面级布局/间距/排版 CSS 暂用 `--mfd-*`（`--id-*` 当前导出的变量集不含间距阶/完整字阶）。
- 不在业务代码里做 `--mfd-*: var(--id-*)` 逐项别名——两套变量完整度不同，假别名会造成半解析状态。
- 收敛路径：设计库补齐语义变量导出并支持皮肤/品牌扩展后，Web 迁移页面级 CSS 到 Indexed token，`tokens.css` 随页面迁移逐段退役；在此之前两套真源并存是登记在案的过渡状态，不视为终态。

## 5. 已迁移页面映射

| 设计 ID | 生产路由 | 真实 API（@mediaflow/api-client-ts） | 采用组件 | 状态与恢复 |
|---|---|---|---|---|
| `WEB-MEDIA-BROWSE` | `/media`（查询参数 `type`/`library_id`/`local_status`/`q`/`cursor`） | `listMediaItems`（游标分页） | `RequestState`、`Button`（提交/清筛选/重试）、原生 select/input（C02/C03 规范原生优先）、页面级 C14 网格卡、`CursorPager` | SSE `catalog.media-changed` 有界刷新；错误/离线 retry 保留筛选并恢复焦点到重试按钮；筛选空提供清筛选；首次空给任务入口 |
| `WEB-MEDIA-DETAIL` | `/media/:id`（返回上下文携带列表查询） | `getMediaItem` | `RequestState`、`MediaArtwork`（暂保留旧包）、原生结构化列表（`dl`/层级/版本文件，Descriptions 本批不采用） | SSE 按 `media_item_id` 匹配刷新；retry 焦点回标题；返回列表保留查询 |

设计规格中的合成能力（评审栏状态切换、DSH 会话抽屉、模拟外部播放等）不进入生产页；未提供正式能力的动作不制造按钮。

## 6. 预算与验证约束

- `check-budget.mjs`（业务入口 gzip ≤ 180 KiB）：首批后 163.2 KB；第二批 AsyncState 委托后 162.9 KB，Modal+Input（含 Dialog/portal）+13.9 KB → **176.8 KB**。剩余约 7.5 KB 余量：后续引入 Select/Table/Drawer 等重组件大概率超出，需先做路由级分包或预算决策，不能默默调预算。
- 本文件所述迁移不代表 L0/G1 或全产品验收；真实 Core 服务、读屏、多浏览器与人工外观验收按既有归属分别记录。
