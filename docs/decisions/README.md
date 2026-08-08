# 架构决策

本目录负责持久的架构决策记录（ADR）。只有当已批准的产品或架构规格尚未覆盖某项决策时，才创建新记录。

记录命名为 `YYYY-MM-DD-short-decision-name.md`，使用英文 ASCII kebab-case。每条记录的状态为 `proposed`、`accepted`、`superseded` 或 `deprecated`，并链接其替代的决策。不得使用 ADR 声称未初始化的实现已经存在。

## 已接受决策

- [使用持久任务与文件操作日志](2026-07-17-use-durable-task-and-file-journals.md)
- [使用能力目录限制文件访问](2026-07-17-use-capability-based-file-access.md)
- [使用类型化内置连接器能力](2026-07-17-use-typed-built-in-connector-capabilities.md)
