# Compose 基础设施

## 目标

定义受支持本地环境的 Compose 部署边界。

## 职责

本地与 NAS Compose 定义。

M2 产物为单服务 `compose.yaml`、部署根样例与 secret 忽略规则。部署值放在未跟踪的 `.env` 和 `secrets/bootstrap.secret` 中；`just check-compose` 必须使用 Docker Compose v2 解析最终配置。

## 非目标

业务逻辑、OCI 镜像定义和厂商安装流程。

## 状态

已初始化：Compose 已在当前 Mac 完成 Docker Compose v2 解析、冷启动、重启、任务与文件结果持久化以及清理验证；目标 NAS 的挂载与权限仍需人工验收。

## 后续验证

`just check-compose`；只读静态验证使用 `just check-compose-static`。
