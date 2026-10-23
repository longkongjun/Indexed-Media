# MediaFlow

MediaFlow 是面向家庭 NAS 的本地优先、自托管数字资源中枢。它负责资源发现、识别、整理、编目和分发，播放与转码由外部产品承担。

## 文档边界

产品、方案、调研、架构基线、决策和路线图位于 [Indexed-wiki](https://github.com/longkongjun/Indexed-wiki)。本仓库只保留代码实现、跨语言契约、开发说明、代码级架构说明和部署实现文档。

- [项目结构](docs/architecture/project-structure.md)
- [归档策略与来源记录](docs/architecture/archive-policy.md)
- [开发文档](docs/development/README.md)
- [运维实现文档](docs/operations/README.md)

## 仓库地图

```text
MediaFlow/
|-- apps/                         # 面向用户交付的正式应用
|   |-- core/                     # Rust Core、API、任务与文件处理
|   |-- web/                      # Vue Web / PWA
|   |-- android/                  # Jetpack Compose 手机与平板客户端
|   |-- ios/                      # SwiftUI 手机与平板客户端
|   |-- android-tv/               # Compose for TV 客户端
|   `-- tvos/                     # SwiftUI tvOS 客户端
|
|-- packages/                     # 可被多个应用复用的共享能力
|   |-- kmp-sdk/                  # Android / iOS 共享业务与数据逻辑
|   |-- api-client-ts/            # Vue 使用的 TypeScript API 客户端
|   |-- api-client-dart/          # Flutter 实验使用的 Dart API 客户端
|   |-- ui-vue/                   # Vue正式基础组件（C01）
|   |-- ui-android/               # Compose正式基础组件
|   |-- ui-android-tv/            # Compose for TV正式基础组件
|   |-- ui-swift/                 # iOS SwiftUI基础组件（验收待补）
|   |-- ui-contract/              # 跨端页面状态与交互约定
|   `-- test-fixtures/            # 跨端共用测试样例
|
|-- contracts/                    # 跨语言接口的唯一事实来源
|   |-- openapi/                  # REST API 契约
|   |-- events/                   # SSE 与异步事件契约
|   `-- examples/                 # 请求、响应与业务场景样例
|
|-- labs/                         # 与正式产品隔离的学习和技术验证
|   |-- compose-multiplatform/    # Compose Multiplatform 垂直切片
|   `-- flutter/                  # Flutter 垂直切片
|
|-- infra/                        # 容器、NAS 部署与可观测性配置
|-- docs/                         # 代码实现、开发和部署相关文档
`-- archive/                      # 公开归档边界说明，不分发内部历史快照
```

正式代码的依赖方向是 `apps -> packages -> contracts`。`labs` 可以使用 `packages` 和 `contracts`，但正式应用与共享包不能依赖 `labs` 或 `archive`。

当前已实现 Rust Core、Vue Web、契约/TS 包、KMP SDK、Android、iOS、JVM 夹具服务及单容器部署入口。TV、Dart、Flutter 等未初始化模块保留独立边界；已实现不等于已经通过真实服务、设备或正式交付验收。

## 检查状态

工程入口、工具环境、测试发现和报告位置见[检查说明](docs/development/checks.md)。

- `just check`：公共源码集合（Rust、契约、TS/UI contract、Web、门禁回归、文档/依赖），不代签原生客户端。
- `just client-check`：客户端完整集合（SDK 全目标、文档、Android、iOS、夹具服务及构建脚本），需要对应平台工具。
- `just check-all`：工程完整集合，加容器镜像构建、Compose 解析和 NAS 静态检查；缺少任何必需平台会失败。跨宿主校准须汇合同基线平台/CI 报告，单机局部成功不代表全量通过。
- `just selfcheck`／`just selfcheck-offline`：源码部署自检，仅公共源码、依赖及部署静态集合；离线 advisory 未执行时明确待验并返回 2，不要求在源码 NAS 上安装 Xcode。
- `just check-m4-source-automation` 保留为既有里程碑子集。真实 NAS、下载器、RSS、模型及真实 Core UI 使用独立 live/验收入口，环境缺失不算通过。

本地项目结构与 AI 工作流门禁由 StudySpace 的 `workflow-packs/mediaflow/` 提供；从 StudySpace 根目录调用对应检查脚本，不向本仓库注入入口或脚本副本。项目仓库的构建与测试入口以根 `justfile` 和各模块文档为准。

项目仓库只保存产品、源码、公开文档、构建和测试能力。本地开发辅助能力由外部工作空间按需提供，不属于项目发布内容。
