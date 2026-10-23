# Android UI foundation

`mediaflow.ui.android` 是公开入口，compileSdk37、最低Android26；沿用仓库AGP/Kotlin/Compose BOM，不依赖KMP SDK、Core、路由或业务实体。API见同目录源码KDoc，暂定evergreen语义值来自`labs/design-system/tokens/tokens.json` v2.0.0。

宿主使用`MfTheme`，通过`dark`控制主题（默认随系统）。`MfButton`的`enabled/busy`受控，异步请求由调用方处理；busy时传入本地化`busyDescription`。`MfContentCard`只接标题/副文/事件/图片槽，装饰图片槽由库清除无障碍语义，宿主保管稳定key、图片失败、导航与完整标题。恢复提示用`MfMessage`；输入、集合、导航、模态直接采用M3/Compose原语。

```kotlin
MfTheme {
    MfButton("Continue", onClick = { /* host event */ })
}
```

构建：根目录`./gradlew :packages:ui-android:assembleRelease`。无业务消费者：`tools/ui-android-consumer`；应用必要消费：Android媒体列表卡/分页按钮/主题。仪器测试：`:packages:ui-android:assembleDebugAndroidTest`后在明确指定的模拟器运行`mediaflow.ui.android.test/androidx.test.runner.AndroidJUnitRunner`。不要默认安装到所有连接设备。

尚未发布。构建/模拟器不能替代TalkBack、真实设备字体/IME/返回、真实服务及应用完整UI验收。
