#!/bin/sh
# 验证镜像构建不变量，并可选地构建和冒烟测试单进程 linux/amd64 容器镜像。
# 输入：可选 `--static-only`；`MEDIAFLOW_TEST_IMAGE` 选择带标签的镜像，`TMPDIR` 选择临时存储位置。
# 输出/副作用：输出诊断信息；在运行时模式下构建镜像并创建/删除临时容器和夹具。
# 退出：成功时为 0；检查器发现条件时 `fail` 退出 1，而 `set -e` 下的直接外部命令（包括 Docker）传播其非零状态；清理仅会将原本成功的运行升级为 1；HUP/INT/TERM 分别映射为 129/130/143。
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
dockerfile="$repo_root/infra/containers/Dockerfile"
dockerignore="$repo_root/.dockerignore"

# 将所有消息参数写入 stderr，并以状态 1 终止入口。
fail() {
  printf 'container check failed: %s\n' "$*" >&2
  exit 1
}

# 要求文件参数 1 包含字面量参数 2；成功时不输出，缺失标记时交由 fail 处理。
require_fixed() {
  file=$1
  text=$2
  grep -F -- "$text" "$file" >/dev/null || fail "$file must contain: $text"
}

[ -f "$dockerfile" ] || fail "$dockerfile is missing"
[ -f "$dockerignore" ] || fail "$dockerignore is missing"

require_fixed "$dockerfile" 'node:24.18.0-bookworm-slim'
require_fixed "$dockerfile" 'rust:1.97.0-bookworm'
require_fixed "$dockerfile" 'debian:bookworm-slim'
require_fixed "$dockerfile" 'pnpm@11.10.0'
require_fixed "$dockerfile" 'pnpm install --frozen-lockfile'
require_fixed "$dockerfile" 'cargo build --manifest-path apps/core/Cargo.toml --locked --release'
require_fixed "$dockerfile" 'USER 10001:10001'
require_fixed "$dockerfile" 'ENTRYPOINT ["/usr/local/bin/mediaflow-core"]'
require_fixed "$dockerfile" 'CMD ["serve"]'
require_fixed "$dockerfile" 'org.opencontainers.image.version="0.2.0-m2"'
require_fixed "$dockerfile" '/usr/lib/apt'
require_fixed "$dockerfile" '/usr/bin/apt*'
require_fixed "$dockerfile" '/usr/bin/dpkg*'
require_fixed "$dockerfile" '/var/lib/dpkg'
require_fixed "$dockerignore" 'infra/compose/secrets/*'
require_fixed "$dockerignore" '*.secret'
runtime_gate=$(sed -n '/^# Docker-backed runtime gate$/,$p' "$0")
for marker in '{{json .Config.Entrypoint}}' '{{json .Config.Cmd}}' '{{json .Config.Healthcheck.Test}}' '{{.State.Health.Status}}' '/api/v1/system/bootstrap-status' '/proc/1/cmdline'; do
  printf '%s\n' "$runtime_gate" | grep -F -- "$marker" >/dev/null \
    || fail "runtime gate must verify: $marker"
done
cleanup_gate=$(sed -n '/^# Root-owned fixture restoration$/,/^# End root-owned fixture restoration$/p' "$0")
for marker in 'host_uid=$(id -u)' 'host_gid=$(id -g)' 'original_status=$?' 'cleanup_error=0' 'trap - EXIT HUP INT TERM' 'chown -R "$1:$2" /fixture' 'chmod -R u+rwX /fixture' 'on_signal()'; do
  printf '%s\n' "$cleanup_gate" | grep -F -- "$marker" >/dev/null \
    || fail "cleanup gate must preserve host removability and exit status: $marker"
done
# Docker 29 的本地容器镜像存储必须获得单平台 manifest，而不是仅保留带证明的索引。
build_command=$(sed -n '/^docker build /p' "$0")
printf '%s\n' "$build_command" | grep -F -- '--provenance=false' >/dev/null \
  || fail 'runtime build must disable provenance for a durable local image tag'

[ "${1:-}" = "--static-only" ] && exit 0

# Docker-backed runtime gate
# 以下为由 Docker 支撑的运行时检查；上一行是自检脚本读取的稳定边界标记。
command -v docker >/dev/null 2>&1 || fail "docker command is required for the image runtime gate"
docker info >/dev/null 2>&1 || fail "docker daemon is required for the image runtime gate"
command -v python3 >/dev/null 2>&1 || fail "python3 is required for same-origin HTTP checks"

# `MEDIAFLOW_TEST_IMAGE` 可选地选择此检查构建和运行的不可变本地标签。
image=${MEDIAFLOW_TEST_IMAGE:-mediaflow:0.2.0-m2}
case "$image" in
  *:latest|latest) fail "test image must use an immutable non-latest tag" ;;
esac

docker build --platform linux/amd64 --provenance=false --file "$dockerfile" --tag "$image" "$repo_root"

architecture=$(docker image inspect --format '{{.Architecture}}' "$image")
[ "$architecture" = "amd64" ] || fail "image architecture is $architecture, expected amd64"

runtime_user=$(docker image inspect --format '{{.Config.User}}' "$image")
case "$runtime_user" in
  ''|0|0:*|root|root:*) fail "image runtime user must be non-root, got: ${runtime_user:-<empty>}" ;;
esac

entrypoint=$(docker image inspect --format '{{json .Config.Entrypoint}}' "$image")
[ "$entrypoint" = '["/usr/local/bin/mediaflow-core"]' ] \
  || fail "unexpected image Entrypoint: $entrypoint"
command=$(docker image inspect --format '{{json .Config.Cmd}}' "$image")
[ "$command" = '["serve"]' ] || fail "unexpected image Cmd: $command"
healthcheck=$(docker image inspect --format '{{json .Config.Healthcheck.Test}}' "$image")
[ "$healthcheck" = '["CMD","/usr/local/bin/healthcheck.sh"]' ] \
  || fail "unexpected image Healthcheck: $healthcheck"

docker run --rm --entrypoint /bin/sh "$image" -eu -c '
  for tool in node pnpm cargo rustc nginx apt apt-get dpkg; do
    if command -v "$tool" >/dev/null 2>&1; then
      printf "forbidden runtime tool found: %s\n" "$tool" >&2
      exit 1
    fi
  done
  test -x /usr/local/bin/mediaflow-core
  test -x /usr/local/bin/healthcheck.sh
  test -f /app/web/index.html
'

# `TMPDIR` 可选地指定创建一次性运行时夹具的位置；默认使用 `/tmp`。
runtime_root=$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-image-check.XXXXXX")
container="mediaflow-image-check-$$"
# Root-owned fixture restoration
# 以下恢复测试夹具的宿主所有权；上一行是自检脚本读取的稳定边界标记。
host_uid=$(id -u)
host_gid=$(id -g)
fixture_restore_required=0
container_cleanup_required=0

# 通过受测镜像将临时运行时夹具递归恢复为调用主机的 UID/GID。
# 无需恢复或恢复成功时返回 0；失败时输出清理诊断并返回 1。
restore_fixture_ownership() {
  if [ "$fixture_restore_required" -eq 0 ]; then
    return 0
  fi
  if ! docker image inspect "$image" >/dev/null 2>&1; then
    printf 'container cleanup failed: image %s is unavailable; cannot restore temporary fixture ownership\n' "$image" >&2
    return 1
  fi
  if ! docker run --rm \
    --user 0:0 \
    --entrypoint /bin/sh \
    --volume "$runtime_root:/fixture" \
    "$image" \
    -eu -c 'chown -R "$1:$2" /fixture; chmod -R u+rwX /fixture' \
    -- "$host_uid" "$host_gid"
  then
    printf 'container cleanup failed: could not restore temporary fixture ownership to %s:%s\n' "$host_uid" "$host_gid" >&2
    return 1
  fi
}

# 删除指定测试容器、恢复主机所有权并删除夹具，同时保留原始状态。
# 清理失败仅会将原本成功的运行升级为状态 1；不会掩盖先前失败。
cleanup() {
  original_status=$?
  trap - EXIT HUP INT TERM
  cleanup_error=0

  if [ "$container_cleanup_required" -eq 1 ]; then
    if ! docker info >/dev/null 2>&1; then
      printf 'container cleanup failed: Docker daemon is unavailable; cannot remove %s\n' "$container" >&2
      cleanup_error=1
    elif docker container inspect "$container" >/dev/null 2>&1; then
      if ! docker rm --force "$container" >/dev/null; then
        printf 'container cleanup failed: could not remove %s\n' "$container" >&2
        cleanup_error=1
      fi
    fi
  fi

  if ! restore_fixture_ownership; then
    cleanup_error=1
  fi
  if ! rm -rf "$runtime_root"; then
    printf 'container cleanup failed: could not remove temporary fixture %s\n' "$runtime_root" >&2
    cleanup_error=1
  fi

  if [ "$original_status" -eq 0 ] && [ "$cleanup_error" -ne 0 ]; then
    original_status=1
  fi
  exit "$original_status"
}

# 将数值信号状态参数 1 转换为进程退出状态，使 EXIT 清理陷阱恰好运行一次。
on_signal() {
  signal_status=$1
  trap - HUP INT TERM
  exit "$signal_status"
}

trap cleanup EXIT
trap 'on_signal 129' HUP
trap 'on_signal 130' INT
trap 'on_signal 143' TERM
# End root-owned fixture restoration
# 上一行结束自检脚本读取的所有权恢复边界。

mkdir -p "$runtime_root/config" "$runtime_root/incoming"
cp "$repo_root/infra/compose/deployment-roots.example.json" "$runtime_root/deployment-roots.json"
printf 'image-check-placeholder\n' >"$runtime_root/bootstrap.secret"
chmod 0755 "$runtime_root/incoming"
chmod 0644 "$runtime_root/deployment-roots.json"
chmod 0600 "$runtime_root/bootstrap.secret"

fixture_restore_required=1
docker run --rm \
  --user 0:0 \
  --entrypoint /bin/sh \
  --volume "$runtime_root:/fixture" \
  "$image" \
  -eu -c 'chown "$1" /fixture/config /fixture/bootstrap.secret; chmod 0700 /fixture/config; chmod 0600 /fixture/bootstrap.secret' \
  -- "$runtime_user"

container_cleanup_required=1
docker run --detach \
  --name "$container" \
  --publish 127.0.0.1::3000 \
  --read-only \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=64m \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --env MEDIAFLOW_PUBLIC_ORIGIN=https://mediaflow.example.test \
  --env MEDIAFLOW_TRUSTED_PROXY_CIDRS=172.16.0.0/12 \
  --volume "$runtime_root/config:/config" \
  --volume "$runtime_root/incoming:/data/incoming:ro" \
  --volume "$runtime_root/deployment-roots.json:/run/mediaflow/deployment-roots.json:ro" \
  --volume "$runtime_root/bootstrap.secret:/run/secrets/mediaflow-bootstrap:ro" \
  "$image" >/dev/null

attempt=0
while [ "$attempt" -lt 50 ]; do
  health_status=$(docker image inspect "$image" >/dev/null 2>&1 && docker inspect --format '{{.State.Health.Status}}' "$container" 2>/dev/null || true)
  case "$health_status" in
    healthy) break ;;
    unhealthy) fail "default Core container became unhealthy" ;;
  esac
  attempt=$((attempt + 1))
  sleep 1
done
[ "$health_status" = healthy ] || fail "default Core container did not become healthy within 50 seconds"

published=$(docker port "$container" 3000/tcp | head -n 1)
port=${published##*:}
case "$port" in
  ''|*[!0-9]*) fail "could not determine the ephemeral HTTP port" ;;
esac

python3 - "$port" <<'PY'
import json
import sys
import urllib.request

base = f"http://127.0.0.1:{sys.argv[1]}"
with urllib.request.urlopen(f"{base}/", timeout=5) as response:
    content_type = response.headers.get_content_type()
    body = response.read().decode("utf-8")
    if response.status != 200 or content_type != "text/html" or '<div id="app">' not in body:
        raise SystemExit("Core did not serve the production Web index")

with urllib.request.urlopen(f"{base}/api/v1/system/bootstrap-status", timeout=5) as response:
    content_type = response.headers.get_content_type()
    value = json.load(response)
    if response.status != 200 or content_type != "application/json":
        raise SystemExit("Core bootstrap-status response is not JSON HTTP 200")
    if value != {"requires_initialization": True, "version": "v1"}:
        raise SystemExit(f"unexpected bootstrap-status payload: {value!r}")
PY

pid_one=$(docker exec "$container" /bin/sh -eu -c 'tr "\000" " " </proc/1/cmdline')
[ "$pid_one" = '/usr/local/bin/mediaflow-core serve ' ] \
  || [ "$pid_one" = '/usr/local/bin/mediaflow-core serve' ] \
  || fail "PID 1 is not the default Core process"

process_attempt=0
while [ "$process_attempt" -lt 5 ]; do
  process_table=$(docker top "$container" -eo pid,comm)
  process_count=$(printf '%s\n' "$process_table" | awk 'NR > 1 { count += 1 } END { print count + 0 }')
  main_command=$(printf '%s\n' "$process_table" | awk 'NR == 2 { print $2 }')
  [ "$process_count" -eq 1 ] && break
  process_attempt=$((process_attempt + 1))
  sleep 1
done
[ "$process_count" -eq 1 ] || fail "default container has $process_count processes; expected one Core process"
[ "$main_command" = mediaflow-core ] || fail "default container process is $main_command, expected mediaflow-core"
