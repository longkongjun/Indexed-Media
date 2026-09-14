# MediaFlow 文档

## 归属

- `product/` 负责已批准的产品定义和技术方向。
- `architecture/` 负责系统边界、仓库结构、可行性研究以及[归档策略与来源记录](architecture/archive-policy.md)；[DSH 家用 NAS 媒体自动化可行性研究](architecture/dsh-home-nas-media-feasibility.md)是外部编排层接入的研究记录。
- `decisions/` 负责架构决策记录。
- `development/` 负责贡献流程和命令指引。
- `operations/` 负责部署与运维文档边界。
- `plans/` 负责已批准的交付计划和产品路线图。

## 命名

新的活跃文档文件名使用英文 ASCII kebab-case。章节索引使用 `README.md`，持久文档使用具有描述性的 kebab-case 名称。应链接既有权威来源，不得复制其持续维护的正文。

当前 Core、Web、契约、TypeScript 客户端、测试夹具和 NAS 单容器部署已初始化；其他模块的实际状态以各目录 README 和根 `justfile` 的验证入口为准。
