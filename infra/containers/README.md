# 容器基础设施

## 目标

定义容器打包边界。

## 职责

OCI 镜像定义。

M2 产物为 `Dockerfile`：Node 24.18.0 与 Rust 1.97.0 多阶段构建，最终 Debian bookworm-slim 层只运行非 root Core。同源 Web/API/SSE 的运行门禁由 `just check-containers` 执行；只读文本检查不能替代镜像构建与 inspect。

## 非目标

业务逻辑、应用源码和编排 manifest。

## 状态

已初始化：提供固定 `linux/amd64` 镜像及构建、inspect、非 root 启动和同源 HTTP 验证入口。实际结果以对应源码和制品的运行证据为准；目标 NAS 仍需人工确认镜像摘要与实际运行状态。

## 后续验证

`just check-containers`；只读静态验证使用 `just check-containers-static`。

可信main流水线一次构建后以`--existing-image`验证并复用同一产物，OCI revision与构建版本绑定完整SHA；正式提升不重建。命令和外部门禁见[镜像发布与空白NAS首次安装](../../docs/operations/image-release-first-install.md)。
