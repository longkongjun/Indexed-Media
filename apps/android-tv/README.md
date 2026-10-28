# Android TV 影视墙

正式 Compose for TV 应用，入口为 `mediaflow.client.tv`。Android TV 与移动端分别持有 UI，复用 KMP SDK；只依赖无业务 Indexed-Design TV 组件。

## 功能与边界

- Web管理端 `/devices/tv-pairing` 签发一次性只读配对码；TV不输入管理员密码。恢复凭据前向Core核对设备身份和权限。
- 影视墙、标题搜索、类型/媒体库/入库状态组合筛选、游标分页、详情和外部播放服务入口。
- 设置显示当前设备名、服务器、只读权限和应用版本；浅色/深色/跟随系统偏好保存在设备本地，退出连接不重置主题。显式退出需确认且默认取消，取消或Back返回原按钮；旧会话确认不会退出新身份。
- 剧集详情按季与集浏览，通用视频成员、根电影和季本身的版本/文件保持各自归属；显示文件相对路径和大小，不打开NAS文件。子节点选择只使用已授权根详情，不将子ID发送到媒体详情端点。
- 同媒体刷新保留当前选择与旧内容，删除选中集退回季、删除季退回根；详情Back逐级返回，顶栏影视墙/设置直接关闭全部详情层级。父级节点、版本和文件阅读使用稳定焦点key及保存的滚动位置，配置重建保留同会话设置路由，身份变更清旧锚点。
- D-pad操作、系统IME、稳定媒体ID焦点恢复。TV不订阅管理SSE，不提供任务/管理写操作，不内置播放器。
- HTTPS默认；可信局域网HTTP需明确启用。Keystore加密文件按服务器隔离，图片仅身份内有界内存缓存；Core派生海报缓存独立于正式资产事实。
- Design AAR和来源固定在 [tv-provenance.json](../../vendor/indexed-design/tv-provenance.json)，不会读取邻仓build目录。

## 验证

`just check-android-tv` 检查固定制品、ktlint、Android lint、JVM测试并编译应用和instrumentation APK。

`./gradlew :apps:android-tv:connectedDebugAndroidTest` 需要已启动且明确选择的Android TV设备；编译成功不等于真实遥控、IME、焦点或三米体验验收。

Windows、Native、NAS、真实提供方/播放器、实体TV和人工验收继续待验；tvOS正式应用另行实施。
