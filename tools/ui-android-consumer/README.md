# Android公开入口消费者

只依赖`:packages:ui-android`和Activity Compose，合成会议场景，无业务SDK、路由、网络或实体。主题切换、预订busy、模拟失败解除busy和卡片事件由宿主持有。

根目录`./gradlew :tools:ui-android-consumer:assembleDebug`；应用ID`example.uifoundation.mobile`。该消费者证明公开入口与受控事件，不代替真实服务验收。
