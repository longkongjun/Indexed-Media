# iOS

## 目标

提供 MediaFlow 的 SwiftUI iPhone/iPad 正式 UI。

## 职责

SwiftUI iPhone/iPad 正式 UI。

## 允许依赖

`packages/ui-swift`、`packages/kmp-sdk`、`packages/ui-contract`、`packages/test-fixtures`、`contracts`。

## 非目标

Core 业务逻辑、Android UI、TV UI、播放以及 `labs` 下的实验。

## 状态

M5 第一批垂直切片已初始化（分支 `feature/m5-client`）：

- **接入**：KMP 经 `MediaFlowKit` framework（iosArm64/iosSimulatorArm64，XcodeGen 工程经 prebuild 脚本链接）+ Kotlin 侧 `MediaFlowIosBridge`（suspend→async 导出、Swift 友好投影、SSE 事件回调）；Swift 层薄 Facade（`AppModel`）+ SwiftUI 六屏（与 Android 同一状态语义五态 + stale）。
- **工程**：工程由 `project.yml` 生成（`xcodegen generate`，产物 gitignore）；Swift 6 严格并发；`.swiftlint.yml` 就位（本机无 CLI，见台账 M5-D-0002）。
- **验证**：`xcodebuild test -scheme MediaFlowiOS`（模拟器 XCTest 3/3：桥存在性/五态语义/未登录优雅错误）；模拟器六屏走查步骤见 `tools/client-fixture-server/README.md`（默认地址 `http://127.0.0.1:8642`）。

## 后续验证

`cd apps/ios && xcodegen generate && xcodebuild test -project MediaFlow.xcodeproj -scheme MediaFlowiOS -destination 'platform=iOS Simulator,name=iPhone 17'`。

## 工程检查

`just client-check-ios` 使用固定版本 SwiftLint、XcodeGen 和 `MediaFlowiOS` 普通 XCTest。工具、模拟器选择与独立日志/xcresult 见[检查说明](../../docs/development/checks.md)。`MediaFlowiOSUI` 的真实 Core UI、真机签名与设备验收是独立出口。

B1基础库消费只覆盖现有媒体列表/主题/分页Action，见[`packages/ui-swift`](../../packages/ui-swift/README.md)。Screen路径到既有目标页的destination注册已补齐，新增正式媒体行进入详情/返回的UI回归用例。完整Xcode/iOS SDK缺失，当前只做语法检查；该回归需要真实Core、Web签发配对码与正式媒体，缺任一项保持待验。这不代表整页高保真、导航运行/真实配对或设备验收已完成。
