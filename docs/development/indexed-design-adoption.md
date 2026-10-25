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
| `MfField` / `MfDialog` / `MfMenu` | 待后续批次 | 登录、审批、收件目录等页面仍在消费；按页面批次逐个映射（`Input`/`Modal`/原生 menu 模式），未迁移页面继续从旧包导入，旧包不删除。 |

共享组件 `CursorPager`（原生按钮）与 `AppShell` 未改：前者本就是原生实现且被 9 个视图共享，后者有待其他会话的未提交修改，均留待对应页面批次。

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

- `check-budget.mjs`（业务入口 gzip ≤ 180 KiB）：接入前 93 KB → provider 基础设施 +68 KB → 迁移两页后 **163.2 KB**。antd 组件按需增量很小（Button/Empty/Spin 合计 ≈1.8 KB），后续页面迁移的主要成本已支付；新增带弹层重组件（Select/Table 等）时仍需复测。
- 本文件所述迁移不代表 L0/G1 或全产品验收；真实 Core 服务、读屏、多浏览器与人工外观验收按既有归属分别记录。
