# Android TV 影视墙

正式 Compose for TV 应用，入口为 `mediaflow.client.tv`。Android TV 与移动端分别持有 UI，复用 KMP SDK；只依赖无业务 Indexed-Design TV 组件。

## 功能与边界

- Web管理端 `/devices/tv-pairing` 签发一次性只读配对码；TV不输入管理员密码。恢复凭据前向Core核对设备身份和权限。
- 影视墙、标题搜索、类型/媒体库/入库状态组合筛选、游标分页、详情和外部播放服务入口。
- 设置显示当前设备名、服务器、只读权限和应用版本；浅色/深色/跟随系统偏好保存在设备本地，退出连接不重置主题。显式退出需确认且默认取消，取消或Back返回原按钮；旧会话确认不会退出新身份。
- D-pad操作、系统IME、稳定媒体ID焦点恢复。TV不订阅管理SSE，不提供任务/管理写操作，不内置播放器。
- HTTPS默认；可信局域网HTTP需明确启用。Keystore加密文件按服务器隔离，图片仅身份内有界内存缓存；Core派生海报缓存独立于正式资产事实。
- Design AAR和来源固定在 [tv-provenance.json](../../vendor/indexed-design/tv-provenance.json)，不会读取邻仓build目录。

## 验证

`just check-android-tv` 检查固定制品、ktlint、Android lint、JVM测试并编译应用和instrumentation APK。

`./gradlew :apps:android-tv:connectedDebugAndroidTest` 需要已启动且明确选择的Android TV设备；编译成功不等于真实遥控、IME、焦点或三米体验验收。

Windows、Native、NAS、真实提供方/播放器、实体TV和人工验收继续待验；tvOS正式应用另行实施。
