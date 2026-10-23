# iOS

MediaFlow 的 SwiftUI iPhone 客户端，最低 iOS 16。KMP 只共享业务与数据，SwiftUI 通过 `MediaFlowKit` 二进制框架和 `MediaFlowUI` Swift Package 构建原生界面；不从同级应用或 labs 导入。iPad 不在当前正式设计与验收范围。

## 模块与依赖

`AppModel` 是 KMP 桥接薄层；SwiftUI 六屏使用五态与 stale 语义。`Screen` 的 destination 注册承载媒体详情、确认与既有入口；媒体列表行、分页 Action、主题从正式 `packages/ui-swift` 公开入口消费。独立组件工作台在 `tools/ui-swift-consumer`，不依赖 KMP 或客户端业务。

允许依赖 `packages/ui-swift`、`packages/kmp-sdk`、`packages/ui-contract`、`packages/test-fixtures` 和 `contracts`。客户端不实现 Core 业务、内建播放器、Android / TV UI。

## 构建与测试

工程由 `project.yml` 生成。SDK 声明 iosArm64 / iosSimulatorArm64；模拟器构建限定 arm64，与实际提供的 KMP 二进制架构一致。完整 Xcode、匹配 iOS SDK、Java 和仓库 Gradle 为构建前提。

```bash
cd apps/ios
xcodegen generate
xcodebuild -project MediaFlow.xcodeproj -scheme MediaFlowiOS -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' build
xcodebuild -project MediaFlow.xcodeproj -scheme MediaFlowiOS -destination 'platform=iOS Simulator,name=iPhone 17' -only-testing:MediaFlowiOSTests test
```

Xcode27 / iOS27 SDK 完整类型编译、应用及普通 XCTest 目标构建通过；iPhone17 / iOS26.2 模拟器普通 XCTest 3 项通过，覆盖桥导出、五态消息、未登录安全错误。生成工程、缓存和报告不入库。SwiftLint、XcodeGen 与测试的集中入口为 `just client-check-ios`，工具条件与独立日志见[工程检查](../../docs/development/checks.md)。

## 运行与人工验收

工具应用提供无服务依赖的主题、公开 API、表单、模态、状态恢复、原生列表和导航检查。业务客户端的 fixture 步骤见 `tools/client-fixture-server/README.md`。`MediaFlowiOSUI` 真实 Core 测试需要独立测试服务、Web 签发的一次性配对码与正式媒体，不在普通 XCTest 范围；不得把合成数据或编译通过登记为真实业务通过。

媒体行 push / 返回、冷恢复、真实配对与撤销、外部播放、真机签名、VoiceOver、Dynamic Type、键盘和手势均须分别验收。尚未实际人工操作的项目保持 **待人工验收**；不采集截图、不运行 AI 逐图或像素差异验收。
