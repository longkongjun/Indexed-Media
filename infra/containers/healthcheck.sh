#!/bin/sh
# 将 OCI 健康检查委托给固定的 `mediaflow-core healthcheck` 命令，并忽略所有脚本参数。
# 输入/环境：忽略脚本参数；此包装器不解析环境变量，而 Core 仍可读取其常规运行时配置。
# 副作用/退出：通过 `exec` 替换 shell 进程，并返回 Core 健康检查状态，包括找不到命令时的失败。
set -eu
exec /usr/local/bin/mediaflow-core healthcheck
