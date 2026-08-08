#!/bin/sh
# 验证 NAS、开发、部署、代理和备份文档保留所需的 M2 声明及待验收表述。
# 输入/环境：不接受参数或环境覆盖；仅读取仓库相对路径的文档，不作修改。
# 输出/副作用：仅输出失败诊断且不写入；全部标记存在时退出 0，任一违规时退出 1。
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
nas="$repo_root/infra/nas/ugreen-dxp4800-plus.md"
development="$repo_root/docs/development/m2-local-development.md"
deployment="$repo_root/docs/operations/m2-deployment.md"
proxy="$repo_root/docs/operations/reverse-proxy.md"
backup="$repo_root/docs/operations/backup-restore.md"

# 将所有消息参数写入 stderr，并以状态 1 终止文档检查入口。
fail() {
  printf 'NAS documentation check failed: %s\n' "$*" >&2
  exit 1
}

# 要求文件参数 1 包含字面量参数 2；成功时不输出，缺失标记时交由 fail 处理。
require_fixed() {
  file=$1
  text=$2
  grep -F -- "$text" "$file" >/dev/null || fail "$file must contain: $text"
}

for file in "$nas" "$development" "$deployment" "$proxy" "$backup"; do
  [ -f "$file" ] || fail "$file is missing"
done

for text in 'DXP 4800 Plus' 'UGOS Pro' 'PUID' 'PGID' '0600' '/config' '/data/incoming' '人工验收' '尚未验证' 'docker buildx build --platform linux/amd64' 'docker image inspect' '.Architecture' '.RepoDigests'; do
  require_fixed "$nas" "$text"
done
for text in 'Node 24.18.0' 'pnpm 11.10.0' 'Rust 1.97.0' 'Docker Compose' '同源'; do
  require_fixed "$development" "$text"
done
for text in 'mediaflow:0.2.0-m2' 'MEDIAFLOW_PUBLIC_ORIGIN' 'MEDIAFLOW_TRUSTED_PROXY_CIDRS' 'bootstrap.secret' 'read-only' '宿主文件的 chown 与 chmod 0600 是权限事实来源' '只读 bind mount'; do
  require_fixed "$deployment" "$text"
done
for text in 'Nginx Proxy Manager' 'SSE' 'proxy_buffering off' 'proxy_read_timeout' 'X-Forwarded-Proto' 'X-Forwarded-Host' 'WebSocket' '不是必需'; do
  require_fixed "$proxy" "$text"
done
for text in '干净' '空 `/config`' 'verify-database' 'restore-backup' 'integrity_check' '外键' '管理员' '收件目录' '任务' 'MEDIAFLOW_CONFIG_PATH' 'MEDIAFLOW_ROOTS_CONFIG_PATH' 'deployment-roots.json' 'manifest' 'SHA-256'; do
  require_fixed "$backup" "$text"
done

if grep -R -F 'NAS 验收已通过' "$nas" "$development" "$deployment" "$proxy" "$backup" >/dev/null; then
  fail "documentation must not claim that unexecuted NAS acceptance passed"
fi
