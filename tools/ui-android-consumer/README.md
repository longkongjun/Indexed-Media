# Android公开入口消费者

用途调整：承接同平台正式基础库的人工组件工具页，现有最小消费者保留。公开API/主题/变体/关键状态展示与人工操作清单尚待补齐；状态为**待人工验收**。构建/自动测试不能代签人工通过，截图AI验收停用。见工程检查说明的“UI交付与人工验收”。

只依赖`:packages:ui-android`和Activity Compose，合成会议场景，无业务SDK、路由、网络或实体。主题切换、预订busy、模拟失败解除busy和卡片事件由宿主持有。

根目录`./gradlew :tools:ui-android-consumer:assembleDebug`；应用ID`example.uifoundation.mobile`。该消费者证明公开入口与受控事件，不代替真实服务验收。
