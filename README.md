# MediaFlow

MediaFlow 是面向家庭 NAS 的本地优先、自托管数字资源中枢。它负责资源发现、识别、整理、编目和分发，播放与转码由外部产品承担。

## 活跃规格

- [产品定义](docs/product/product-definition.md)
- [项目结构](docs/architecture/project-structure.md)
- [归档策略与来源记录](docs/architecture/archive-policy.md)
- [完整路线图](docs/plans/roadmap.md)

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
|-- docs/                         # 产品、架构、决策、计划和运维文档
|-- tools/                        # 产品构建、测试和运维工具
`-- archive/                      # 公开归档边界说明，不分发内部历史快照
```

正式代码的依赖方向是 `apps -> packages -> contracts`。`labs` 可以使用 `packages` 和 `contracts`，但正式应用与共享包不能依赖 `labs` 或 `archive`。

当前实现以 Rust Core、Vue Web、版本化 OpenAPI/事件契约、TypeScript 客户端、测试夹具和单容器 NAS 部署为主。Android、iOS、TV、KMP、Flutter 与部分实验模块仍保留目录边界，尚未进入正式交付范围。

## 检查状态

公开仓库的默认自动门禁通过以下命令运行：

- `just check`：执行 Core 格式、Clippy、全目标测试、契约、客户端、共享夹具、Web 类型/测试/构建/E2E 与差异检查。
- `just check-m4-source-automation`：与默认检查相同的当前里程碑聚合入口。
- 真实 NAS、下载器、RSS 与模型验收使用对应 `*-live` 命令，环境缺失时不得冒充通过。

本地项目结构与 AI 工作流门禁由 StudySpace 注入，使用 `./tools/project-layout/check.sh` 和 `./tools/workflow/check.sh`，不属于公开仓库内容。

项目仓库只保存产品、源码、公开文档、构建和测试能力。本地开发辅助能力由外部工作空间按需提供，不属于项目发布内容。
