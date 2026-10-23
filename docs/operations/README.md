# 运维文档

本目录负责部署、备份、升级、可观测性、诊断和 NAS 运维文档。相关模块初始化后，运行时与部署定义放在 `infra/`；本索引本身不表示实现已经存在。

新的持久文档使用英文 ASCII kebab-case 文件名，并链接权威产品或架构来源，不得复制其约束正文。
- [实机验收 Runbook](acceptance-runbook.md)：从开发机自检到 NAS 部署与延后验收项的完整步骤

- [镜像发布与空白NAS首次安装](image-release-first-install.md)：可信候选、同digest提升、完整发布包和仅首装入口；已有实例升级/恢复不在此入口。
