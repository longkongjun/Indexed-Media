# Foundation studio · Android

独立组件与基础规范应用，最低 Android 26，公开消费正式库。仅使用本地合成数据，不依赖 KMP SDK、Core、业务实体、账户或真实配置。所有未实际人工操作项均为 **待人工验收**。构建、行为测试、真实设备与人工验收分别记录；不采集截图或运行像素比较。

## 构建、安装与启动

从代码仓库根运行：

```bash
./gradlew :tools:ui-android-consumer:assembleDebug
adb -s <device-serial> install -r tools/ui-android-consumer/build/outputs/apk/debug/ui-android-consumer-debug.apk
adb -s <device-serial> shell am start -n example.uifoundation.mobile/example.uifoundation.mobile.MainActivity
```

只安装到明确选择的目标。TV 应用使用 Leanback 启动入口，真实遥控、系统密度和远距阅读须在 Android TV 系统上验收，普通手机模拟器不替代。

## 公开入口与状态所有者

`mediaflow.ui.android` 提供 `MfTheme`、`MfButton`、`MfContentCard`、`MfMessage`。应用显示完整调用参数与责任说明。输入、Switch、Checkbox、FilterChip、滚动集合与 AlertDialog 直接使用 Material 3 / Compose 成熟原语，不制造同名包装。主题、草稿、筛选、展开、当前对象与请求状态均由消费者持有，有限 UI 值使用 rememberSaveable。

## 人工操作

1. 切换浅/暗主题，检查正文、辅助文字、内容卡和按钮；放大系统字体，检查纵向滚动、触达与安全区。
2. 激活 Reserve seat，观察名称保留、Attempts 只加一次、busy 期间不能重复；模拟失败后可重试；Unavailable 不能激活。
3. 激活长标题卡并返回，完整标题可读；缺图不移除名称。
4. 编辑 Display name、切换选项并旋转，检查草稿和选择保留；使用真实键盘/IME 和 TalkBack 检查名称及阅读顺序。
5. 打开 Edit modal，空保存显示错误；填值后保存进入 busy；取消/返回不关闭；Fail save 保留草稿；Cancel 后 Continue editing 保值，Discard draft 才清空。
6. 选择不同请求状态，读取恢复说明；展开/收起完整信息，Load more 不替换之前内容。

页面是组件工具应用；按钮模拟请求，状态预览不构成任何真实业务结果。TalkBack、系统键盘与真实设备验收仍由人工完成。

## C01–C17 采用映射

“正式实现”指公开基础库 API；“原生采用”指平台原语由应用组合，不承诺同名库组件。目录说明不等于所有原生状态已运行通过。

| 组 | 正式实现或原生采用 | 工具应用中的验证入口 |
|---|---|---|
| C01 操作 | `MfButton` 正式实现 | busy、禁用、失败后重试 |
| C02 输入 | M3 `OutlinedTextField` 原生采用 | Display name、空校验与失败保值 |
| C03 选择 | M3 `DropdownMenu` 原生采用 | Filter / 选择菜单 |
| C04 勾选与开关 | M3 `Checkbox` / `Switch` 原生采用 | 本地预览开关 |
| C05 筛选 | M3 `FilterChip` 原生采用 | All / Upcoming 或 Presentation |
| C06 状态标签 | 原生语义文字组合 | 本地数据与请求状态文字 |
| C07 信息行 | `MfContentCard` 正式实现及原生文字 | 完整标题与副文 |
| C08 集合 | Compose 滚动 `Column` 原生采用 | 可滚动组件目录；生产数据分页另验 |
| C09 导航 | 稳定对象与本地返回事件 宿主组合 | 详情返回；真实客户端路由另验 |
| C10 模态 | M3 `AlertDialog` 原生采用 | dirty / busy / 失败 / 显式丢弃 |
| C11 常驻反馈 | `MfMessage` 正式实现 | 失败说明与清反馈 |
| C12 请求状态 | `MfMessage` 及宿主受控状态 | loading / empty / no match / offline / error / stale 本地预览 |
| C13 图像 | 宿主图像槽 / 原生加载能力 | 装饰槽与缺图标题；网络图片链路另验 |
| C14 内容 | `MfContentCard` 正式实现 | 长标题与缺图内容 |
| C15 展开 | 受控展开状态与 Compose 内容 原生采用或宿主组合 | 完整信息展开/收起 |
| C16 更多 | `MfButton` 与宿主稳定集合 | busy 防重复；真实游标追加另验 |
| C17 菜单 | M3 `DropdownMenu` 原生采用 | 选择或请求状态菜单 |

公开 API 的主题和状态由消费者控制。无真实数据源的状态仅用来人工检查呈现与恢复，不证明生产请求、认证、离线缓存、分页或图片服务可用。平台字体、读屏、键盘、返回和真实设备须逐项人工记录。
