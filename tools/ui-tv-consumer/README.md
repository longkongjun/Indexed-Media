# TV公开入口消费者

只依赖`:packages:ui-android-tv`和Activity Compose。合成会议条目，无网络/SDK/MediaFlow实体。D-pad移动、确认进详情、Back返回原ID；删除当前焦点条目后选最近存活项，空集提供重置。由测试宿主控制这些状态，不是库内业务导航，也不是正式TV客户端。

`./gradlew :tools:ui-tv-consumer:assembleDebug`。只安装到明确选定的测试模拟器；TV系统与真实遥控另验。
