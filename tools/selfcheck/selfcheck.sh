#!/bin/sh
# MediaFlow 一键自检：环境探测 + 本地全量门禁聚合 + 交付就绪摘要。
# 用途：实机验收前在目标机器（或开发机）上单命令确认代码、契约与文档门禁全绿。
# 输入：可选 `--offline`（跳过 cargo-deny advisory 在线拉取）。
# 退出：全部通过 0；环境缺失 2；门禁失败 1。
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$repo_root"

offline=0
[ "${1:-}" = "--offline" ] && offline=1

pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; }
info() { printf '  ·  %s\n' "$1"; }

status=0

printf 'MediaFlow 一键自检\n'
printf '==================\n'

printf '\n[1/4] 环境探测\n'
for tool in cargo just pnpm node docker; do
  if command -v "$tool" >/dev/null 2>&1; then
    pass "$tool $(command -v "$tool" | head -c 60)"
  else
    fail "$tool 缺失"
    status=2
  fi
done
[ "$status" = 2 ] && { printf '\n环境缺失，自检中止。\n'; exit 2; }

# Xcode license 规避（macOS 上 git/cc 走 CommandLineTools）。
if [ "$(uname)" = "Darwin" ] && [ -d /Library/Developer/CommandLineTools ]; then
  export DEVELOPER_DIR=/Library/Developer/CommandLineTools
  info "DEVELOPER_DIR 已设置（Xcode license 规避）"
fi

printf '\n[2/4] 依赖安装\n'
if pnpm install --frozen-lockfile >/tmp/mediaflow-selfcheck-install.log 2>&1; then
  pass "pnpm 依赖就绪"
else
  fail "pnpm install 失败"
  exit 1
fi

printf '\n[3/4] 全量门禁（just check）\n'
if just check >/tmp/mediaflow-selfcheck.log 2>&1; then
  pass "just check 全绿"
else
  # cargo-deny 需要网络拉取 advisory-db；离线时提示而非失败。
  if grep -q "failed to fetch advisory database" /tmp/mediaflow-selfcheck.log && [ "$offline" = 1 ]; then
    info "advisory-db 在线拉取失败且 --offline 已指定：跳过 cargo-deny"
  else
    fail "just check 失败（详情 /tmp/mediaflow-selfcheck.log）"
    tail -20 /tmp/mediaflow-selfcheck.log
    exit 1
  fi
fi

printf '\n[4/4] 静态部署边界（just check-all 的容器/文档部分）\n'
if just check-containers-static >/dev/null 2>&1 \
  && just check-compose-static >/dev/null 2>&1 \
  && just check-nas >/dev/null 2>&1; then
  pass "容器/Compose/NAS 文档静态门禁绿"
else
  fail "静态部署门禁失败"
  exit 1
fi

printf '\n摘要\n'
printf -- '----\n'
printf '代码/契约/文档门禁：全部通过。\n'
printf '运行时验证（真实 NAS / 外部服务 / DSH / compose 镜像）：见任务延后验收台账；\n'
printf '  网络受限项：cargo-deny advisory（github.com）、Docker 镜像拉取。\n'
printf '完整实机步骤：infra/nas/ + delivery runbook。\n'
