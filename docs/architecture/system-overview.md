# MediaFlow 系统概览

MediaFlow 是本地优先、自托管的模块化单体。权威产品边界见[产品定义](../product/product-definition.md)，MVP 详细领域模型、模块协作、文件安全、任务恢复、部署和性能预算见[MVP 架构基线](mvp-architecture-baseline.md)，仓库边界见[项目结构规格](project-structure.md)，历史内容约束见[归档策略与来源记录](archive-policy.md)。

## Core 模块

- `catalog`
- `discovery`
- `identification`
- `organization`
- `tasks`
- `connectors`
- `identity`
- `admin`

模块之间通过明确应用接口协作，不跨模块访问 repository 或数据库表。Core 使用持久任务协调处理流程，文件影响通过持久操作日志恢复；连接器按类型化能力隔离。

具体数据库 Schema、REST/OpenAPI、事件和代码接口由对应版本化设计与实现定义。Rust Core 与 Vue Web 已承载当前正式实现；移动端、TV 与隔离实验仍按各模块 README 标注的状态演进。
