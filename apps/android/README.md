# Android

## 目标

提供 MediaFlow 的 Jetpack Compose 手机和平板正式 UI。

## 职责

Jetpack Compose 手机和平板正式 UI。

## 允许依赖

`packages/kmp-sdk`、`packages/ui-contract`、`packages/test-fixtures`、`contracts`。

## 非目标

Core 业务逻辑、TV UI、iOS UI、播放以及 `labs` 下的实验。

## 状态

M5 第一批垂直切片已初始化（分支 `feature/m5-client`）：

- **六屏**：登录 → 媒体墙（分页加载 + 离线缓存首屏）→ 详情（元数据/版本/分集 + 外部播放跳转）→ 人工确认（队列 + 候选搜索 + 决定提交）→ 任务控制（列表 + 取消）；页面状态语义（Loading/Content/Empty/Error/Offline + stale）经 `PageVisual` 统一映射（对齐 ui-contract / Web `TaskCenterState`）。
- **架构**：`SdkContainer` 组装 SDK（登录页配置 base URL）；ViewModel 层持有分页/SSE 刷新桥；无导航框架（返回栈由 AppRoot 管理）。
- **门禁**：`just client-check-android`（assembleDebug + 单测）；fixture 驱动走查：`just client-fixture-server` 后在模拟器登录（默认地址 `http://10.0.2.2:8642`，密码 `fixture`）。
- **凭据存储**：第一批文件落地（应用私有目录）；Keystore 加密随第二批设备认证接入。

## 后续验证

`just client-check-android`；模拟器走查步骤见 `tools/client-fixture-server/README.md`。
