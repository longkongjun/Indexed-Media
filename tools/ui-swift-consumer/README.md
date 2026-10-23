# Foundation studio · iOS

独立原生组件与基础规范应用，iOS 16+。只依赖 `../../packages/ui-swift` 的 `MediaFlowUI` product，不依赖 KMP、网络、业务实体、账户或真实配置。所有数据和请求均为本地合成示例。未实际人工操作项均为 **待人工验收**；构建、行为测试、设备/服务与人工验收分别记录。不采集截图或运行像素比较。

## 构建、安装与启动

从代码仓库根运行：

```bash
xcodegen generate --spec tools/ui-swift-consumer/project.yml
xcodebuild -project tools/ui-swift-consumer/UIFoundationConsumer.xcodeproj -scheme UIFoundationConsumer -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/mediaflow-foundation-swift CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- build
xcrun simctl install <device-UDID> /tmp/mediaflow-foundation-swift/Build/Products/Debug-iphonesimulator/UIFoundationConsumer.app
xcrun simctl launch <device-UDID> example.uifoundation.swift
```

使用完整 Xcode，模拟器须已启动；真机在 Xcode 中设置签名后安装。自动行为测试为同 scheme 的 UIFoundationConsumerTests；它检查 busy、重复事件与恢复，不替代人工验收。生成工程是本地构建产物。

## 公开 API 与原生采用

`MfTheme { content }` 使用系统颜色方案；`MfButton(title, enabled, busy, busyDescription, action)`、`MfContentRow(title, subtitle, action, artwork)` 与 `MfMessage(message, action)` 从公开模块直接消费。主题、请求、草稿与导航由应用持有。输入、选择、列表、导航、模态、菜单、展开直接采用 SwiftUI TextField / Toggle / Picker / List / NavigationStack / sheet / Menu / DisclosureGroup，不复制同名包壳。

## 人工操作

1. 选择 System、Light、Dark，检查主题和系统字体；使用 Dynamic Type、VoiceOver 及外接键盘检查阅读和焦点顺序。
2. Reserve seat 后名称保留、Attempts 加一次，busy 禁重复；Simulate failure 恢复重试；Unavailable 始终禁用。
3. 打开长标题或缺图行，详情读取完整标题并系统返回，装饰图片不重复播报名称。
4. 编辑 Display name、选择 Presentation / Filter；打开 Edit modal，空保存显示错误，填值保存进入 busy，Cancel 和手势关闭受阻；Fail save 保留草稿，Cancel 后可继续编辑或显式丢弃。
5. 切换请求状态并清反馈，展开/收起完整内容，检查大字下内容自然增高和安全区/键盘避让。

真实设备 VoiceOver、Dynamic Type、键盘与手势仍须人工验证；工具应用不代表生产客户端真实认证或业务链路通过。

## C01–C17 采用映射

“正式实现”指公开基础库 API；“原生采用”指平台原语由应用组合，不承诺同名库组件。目录说明不等于所有原生状态已运行通过。

| 组 | 正式实现或原生采用 | 工具应用中的验证入口 |
|---|---|---|
| C01 操作 | `MfButton` 正式实现 | busy、禁用、失败后重试 |
| C02 输入 | SwiftUI `TextField` 原生采用 | Display name、空校验与失败保值 |
| C03 选择 | SwiftUI `Picker` 原生采用 | Filter / 选择菜单 |
| C04 勾选与开关 | SwiftUI `Toggle` 原生采用 | 本地预览开关 |
| C05 筛选 | SwiftUI segmented `Picker` 原生采用 | All / Upcoming 或 Presentation |
| C06 状态标签 | 原生语义文字组合 | 本地数据与请求状态文字 |
| C07 信息行 | `MfContentRow` 正式实现及原生文字 | 完整标题与副文 |
| C08 集合 | SwiftUI `List` / `Section` 原生采用 | 可滚动组件目录；生产数据分页另验 |
| C09 导航 | SwiftUI `NavigationStack` 宿主组合 | 详情返回；真实客户端路由另验 |
| C10 模态 | SwiftUI `sheet` / `alert` 原生采用 | dirty / busy / 失败 / 显式丢弃 |
| C11 常驻反馈 | `MfMessage` 正式实现 | 失败说明与清反馈 |
| C12 请求状态 | `MfMessage` 及宿主受控状态 | loading / empty / no match / offline / error / stale 本地预览 |
| C13 图像 | 宿主图像槽 / 原生加载能力 | 装饰槽与缺图标题；网络图片链路另验 |
| C14 内容 | `MfContentRow` 正式实现 | 长标题与缺图内容 |
| C15 展开 | SwiftUI `DisclosureGroup` 原生采用或宿主组合 | 完整信息展开/收起 |
| C16 更多 | `MfButton` 与宿主稳定集合 | busy 防重复；真实游标追加另验 |
| C17 菜单 | SwiftUI `Menu` 原生采用 | 选择或请求状态菜单 |

公开 API 的主题和状态由消费者控制。无真实数据源的状态仅用来人工检查呈现与恢复，不证明生产请求、认证、离线缓存、分页或图片服务可用。平台字体、读屏、键盘、返回和真实设备须逐项人工记录。
