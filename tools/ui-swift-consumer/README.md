# SwiftUI公开入口消费者

XcodeGen工程只依赖`../../packages/ui-swift`的`MediaFlowUI` product，无KMP/网络/业务实体。会议预订的busy/失败/重试由本地状态驱动；UITests验证名称、禁重复及恢复。

在本目录`xcodegen generate`，使用完整Xcode运行`UIFoundationConsumer` scheme。iOS16+。生成声明与语法检查不能代替iOS编译或UI测试；需要完整且匹配的Xcode/iOS SDK。
