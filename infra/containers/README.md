# 容器基础设施

## 目标

定义容器打包边界。

## 职责

OCI 镜像定义。

M2 产物为 `Dockerfile`：Node 24.18.0 与 Rust 1.97.0 多阶段构建，最终 Debian bookworm-slim 层只运行非 root Core。同源 Web/API/SSE 的运行门禁由 `just check-containers` 执行；只读文本检查不能替代镜像构建与 inspect。

## 非目标

业务逻辑、应用源码和编排 manifest。

## 状态

已初始化：固定 `linux/amd64` 镜像已在当前 Mac 完成构建、inspect、非 root 启动和同源 HTTP 验证；目标 NAS 仍需人工确认镜像摘要与实际运行状态。

## 后续验证

`just check-containers`；只读静态验证使用 `just check-containers-static`。
