# MediaFlow 项目结构

MediaFlow 使用 Monorepo 管理正式应用、共享包、跨语言契约、基础设施和隔离实验。项目仓库只包含能够公开发布的产品内容；个人开发环境和本地自动化由外部工作空间提供。

## 顶层目录

```text
MediaFlow/
├── apps/          正式应用
├── packages/      跨应用共享包
├── contracts/     REST、事件和场景契约
├── infra/         部署与可观测性配置
├── labs/          与正式产品隔离的实验
├── docs/          产品、架构、决策和开发文档
├── tools/         项目构建与验证工具
└── archive/       重建前只读历史
```

## 依赖方向

- `apps` 可以依赖 `packages` 和 `contracts`。
- `packages` 可以依赖 `contracts`，不能依赖 `apps`。
- 正式代码不能依赖 `labs` 或 `archive`。
- Core 不提供播放、转码、站点聚合或必需的云端控制面。
- 正式 Web 使用 Vue；Android 使用 Jetpack Compose；iOS 使用 SwiftUI。
- KMP 共享业务与数据逻辑，不共享正式 UI。

## 模块边界

每个已初始化模块通过自身 README 说明职责、公共接口、依赖和验证命令。跨语言行为以 `contracts/` 为事实来源；产品定义、系统架构和架构决策分别位于 `docs/product/`、`docs/architecture/` 和 `docs/decisions/`。

## 本地开发辅助

个人编辑器配置和本地任务状态不属于项目发布内容。初始化任何本地辅助配置前后，`git status` 都必须保持干净。
