#!/bin/sh
# Source/deployment-static checks; offline advisory evidence remains pending (2).
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$repo_root"
offline=0
case "$*" in
  '') ;;
  --offline) offline=1 ;;
  *) printf 'usage: %s [--offline]\n' "$0" >&2; exit 2 ;;
esac

output_root=$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-selfcheck.XXXXXX")
printf '源码部署自检（公共源码、依赖、部署静态检查）\n报告：%s\n' "$output_root"
failed=0
pending=0
for tool in cargo just pnpm node cargo-deny cargo-machete; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf '缺少工具：%s\n' "$tool" >&2
    pending=1
  fi
done
if [ "$pending" = 1 ]; then
  printf '环境缺失，源码自检待验。\n'
  exit 2
fi

# Each independent check runs even if an earlier check failed.
run_check() {
  name=$1
  shift
  if "$@" >"$output_root/$name.log" 2>&1; then
    result=0
    printf '通过：%s\n' "$name"
  else
    result=$?
    failed=1
    printf '失败：%s（退出 %s，日志 %s/%s.log）\n' "$name" "$result" "$output_root" "$name" >&2
    tail -20 "$output_root/$name.log" >&2
  fi
  printf '%s\t%s\n' "$name" "$result" >>"$output_root/status.tsv"
}

run_check install pnpm install --frozen-lockfile
run_check source just check-source
run_check dependencies just check-rust-deps-static
if [ "$offline" = 1 ]; then
  pending=1
  printf '待验：离线模式未执行 advisory 在线检查。\n'
  printf 'advisories\tPENDING\n' >>"$output_root/status.tsv"
else
  run_check advisories just check-rust-advisories
fi
run_check containers just check-containers-static
run_check compose just check-compose-static
run_check nas just check-nas

printf '源码自检报告：%s；真实 NAS、外部服务和客户端验收另行执行。\n' "$output_root"
[ "$failed" = 0 ] || exit 1
[ "$pending" = 0 ] || exit 2
printf '本入口声明的源码与部署静态集合通过。\n'
