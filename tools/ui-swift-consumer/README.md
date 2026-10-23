# SwiftUI公开入口消费者

用途调整：承接同平台正式基础库的人工组件工具页，现有最小消费者保留。公开API/主题/变体/关键状态展示与人工操作清单尚待补齐；状态为**待人工验收**。构建/自动测试不能代签人工通过，截图AI验收停用。见工程检查说明的“UI交付与人工验收”。

XcodeGen工程只依赖`../../packages/ui-swift`的`MediaFlowUI` product，无KMP/网络/业务实体。会议预订的busy/失败/重试由本地状态驱动；UITests验证名称、禁重复及恢复。

在本目录`xcodegen generate`，使用完整Xcode运行`UIFoundationConsumer` scheme。iOS16+。生成声明与语法检查不能代替iOS编译或UI测试；需要完整且匹配的Xcode/iOS SDK。
