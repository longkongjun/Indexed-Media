# MediaFlowUI

公开Swift Package product/module `MediaFlowUI`，iOS16+、Swift6，无第三方依赖。包不导入KMP SDK、业务实体、路由或权限，不联网、不保存主题偏好。

`MfTheme`跟随环境colorScheme，提供暂定evergreen tint/画布；系统文字保原生动态色。`MfButton`主操作接普通文字、启用/busy与闭包，busy保名称并禁重复；宿主提供本地化busyDescription并持有异步结果。`MfContentRow`是List内容行，接普通title/subtitle与图片槽/事件；稳定key/图片状态/完整详情归宿主。`MfMessage`是常驻恢复组合。导航、模态与集合直接采用SwiftUI。

```swift
import MediaFlowUI

MfTheme {
    MfButton("Continue") { /* host event */ }
}
```

独立消费者：`tools/ui-swift-consumer`；生成后用完整Xcode/iOS SDK构建和运行UI测试。MediaFlow应用通过`apps/ios/project.yml`的本地Package路径依赖；不复制源码，不依赖其他worktree。

构建与UI测试需要完整且匹配的Xcode/iOS SDK；语法检查/XcodeGen不等于SwiftUI类型检查、iOS构建或模拟器通过。现有iOS应用的导航destination、真实配对管理依赖与VoiceOver/Dynamic Type需另行验收。尚未发布。
