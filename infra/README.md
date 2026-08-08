# 基础设施

## 目标

为自托管 MediaFlow 提供部署与运维边界。

## 职责

只负责基础设施定义和运维配置；基础设施不包含业务逻辑。

M2 产物分布在 `containers/`、`compose/` 和 `nas/`；各目录保留独立定向门禁，`just check-infra` 汇总本机 Docker 运行验证。

## 非目标

Core 产品行为、应用 UI、契约定义和运行时业务规则。

## 状态

已初始化：单镜像、单 Compose 服务、显式能力根和 NAS 运维文档已具备，并已在当前 Mac 完成 Docker runtime、冷启动、重启与持久化验证；目标 UGREEN DXP 4800 Plus 人工验收待执行。

## 后续验证

`just check-infra`
