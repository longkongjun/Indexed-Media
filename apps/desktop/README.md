# CMP Desktop companion

Mac/Windows 单窗口伴侣：设备配对/凭据 enrollment、冷启动凭据恢复、媒体墙分页/缓存、详情与外部播放、任务和人工确认。移动客户端仍使用 Compose/SwiftUI；桌面直接消费 KMP SDK JVM 目标，不依赖 Android 或 iOS 页面。

锁定 CMP 1.12.2 的桌面框架下限为 macOS 13 arm64、Windows 10（64位 x86-64/arm64），见[官方支持矩阵](https://kotlinlang.org/docs/multiplatform/compose-compatibility-and-versioning.html)。Mac 包显式声明最低系统 13.0；Intel Mac 不在此版本支持范围。框架支持不代表本应用各系统/CPU组合已经验收，Windows与系统交互仍须独立验证。

## Build and run

需要 JDK 21、项目 Gradle wrapper 和网络可解析的锁定依赖。Design 库来自本仓库 `vendor/indexed-design/maven`，其来源 commit/三个产物 SHA-256 记录于 `desktop-provenance.json`；不依赖邻仓或已发布的公共 Maven 包。

```sh
just client-check-desktop
./gradlew :apps:desktop:run
just package-desktop
./gradlew :apps:desktop:packageDistributionForCurrentOS
```

应用内“文件 → 刷新”支持 Mac Command-R / Windows Ctrl-R。窗口宽度足够时媒体与审核页面采用主从布局，窄窗返回列表；确认框初始焦点为取消。输入、按钮与弹层使用 CMP 官方原语，主题和五态/stale 组件使用 `dev.indexed.design:compose-desktop:0.15.0`。

原生发行包含运行时，无需用户预装 JDK；Mac/Windows 分别生成对应宿主的包。产品版本为 0.1.0，Mac jpackage 要求首段大于零，因此仅 Mac 包版本映射为 1.1.0。使用支持 jpackage 的非 Homebrew JDK；CMP vendor 检查不绕过。签名、公证、Windows 安装和正式分发均需独立验证，创建目录不代表安装验收。

## Session and platform boundary

- `DesktopSession` 装配 Repository、单上游 SSE 与驱动；事件先保存游标再广播，所有页面共享同一连接。同步 SQLite 与分页访问走串行 IO，UI 收集 StateFlow。
- `DesktopModel` 负责桌面状态；会话/页面代次 guard 与生命周期 Job 隔离旧响应。退出、401 和关闭清理资源；关闭等待候选登录及会话任务完成清理，显式登出被窗口关闭打断时仍清除本地凭据。退出服务端请求完成前保持忙碌，避免新登录被旧清理关闭。
- `DeviceAuthStore` 实现位于桌面：JNA 5.19.1 调用 macOS Keychain Services、Windows Credential Manager，凭据不进入命令行、日志或配置文件。系统存储失败不进入已登录状态，也不明文降级。
- 配置只保存规范化服务器地址，位于用户 Application Support/LOCALAPPDATA；缓存按服务器和设备 ID 的摘要分文件，防止切换服务器混用。
- 人工确认沿用 SDK 的版本前提和幂等请求。未知回执保留同一次操作键并阻止新决定/案件切换；任务取消亦沿用同键重试。401 清凭据，409 重取案件，Core 的任务与本地窗口生命周期独立。
- 恢复窗口焦点、切换页面和手动刷新会重新核对列表及当前详情；同一案件刷新保留查询草稿，写操作忙碌或回执未知时保留原操作上下文。窗口焦点监听随 UI 生命周期移除。
- 外部播放仅打开 SDK 提供的 HTTP/HTTPS 地址，不执行 shell，也不接管播放/转码。

## Checks and limits

桌面行为测试使用本机 HTTP fixture、真实 SDK/SQLite/事件连接及隔离目录；不代表真实 NAS 或业务权限验收。NativeVaultTest 只创建唯一随机测试服务与非敏感 sentinel，在 finally 删除；系统安全存储不可用时应失败，不降级跳过。非支持 OS 的该测试明确 skipped。

实际验证见 Personal Forge 当前 change 的 evidence；仓库文档不维护跨会话完成状态。`desktop-ci` 配置覆盖 Mac/Windows，但配置存在不代表远端运行过。人工检查需分别覆盖 IME、Tab/Shift-Tab、Esc/取消焦点、缩放、长内容、读屏、窗口关闭/恢复和真实 Core 登录、审核与播放跳转，不做截图或像素比较。

首版不包含多窗口、托盘驻留、自动更新、完整 Web 管理、离线写入队列或移动 UI 迁移。进程重启后重新查询服务端权威状态；内存幂等键不充当跨进程操作日志。
