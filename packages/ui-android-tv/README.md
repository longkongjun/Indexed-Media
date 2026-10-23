# Android TV UI foundation

`mediaflow.ui.tv` 是公开入口，compileSdk37、Android26+，Compose for TV `androidx.tv:tv-material:1.1.0`，沿用仓库Kotlin/BOM。不依赖手机UI库、KMP SDK、业务实体、路由或权限；原生身份/后端只读scope不由UI库裁定。

`MfTvTheme(dark=true)`默认暗色；按钮labelLarge及正文20sp，标题31/39sp。`MfTvButton`保留busy名称并禁重复事件，`MfTvContentCard`接受普通标题/副文/图片槽/事件，`MfTvMessage`承载宿主传入的恢复动作。焦点为3dp `color-border-focus`原生边框（浅#0b7a50/暗#5cd39a），外移4dp留出画布间隔，scale固定1；选中与焦点分开，导航/集合/稳定ID恢复由宿主持有。装饰图片槽由库清除无障碍语义；图片加载不进入库。

```kotlin
MfTvTheme {
    MfTvButton("Continue", onClick = { /* host event */ })
}
```

独立构建：`./gradlew :packages:ui-android-tv:assembleRelease`；独立组件与规范应用：[Foundation studio TV](../../tools/ui-tv-consumer/README.md)。该独立 Foundation studio 应用展示公开 API、主题、受控操作、长标题卡片和稳定焦点恢复，安装启动与人工清单见工具应用 README。正式 TV 产品接入依赖服务端只读授权；现有管理员设备令牌不能仅靠隐藏写操作当作只读设备。测试入口：`:packages:ui-android-tv:assembleDebugAndroidTest`，仅在指定模拟器运行`mediaflow.ui.tv.test/androidx.test.runner.AndroidJUnitRunner`。TV激活使用D-pad确认，触屏点击不是等价输入。

尚未发布。通用Android模拟器上的TV控件与D-pad测试不证明Android TV系统、真实遥控、长按、3m阅读、1080p/4K硬件或外部播放通过；tvOS没有本期实现。
