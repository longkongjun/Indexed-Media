#!/bin/sh
# 验证 Compose 清单和示例，并可选执行由 Docker 支撑的 UID/GID 与密钥挂载检查。
# 输入：可选 `--static-only`；`MEDIAFLOW_TEST_IMAGE` 选择预构建的带标签镜像，`TMPDIR` 选择临时存储位置。
# 输出/副作用：输出诊断信息；运行时模式会创建/删除临时 Compose 项目，且绝不打印密钥内容。
# 退出：成功时为 0；检查器发现条件时 `fail` 退出 1，而 `set -e` 下的直接外部命令（包括 Docker）传播其非零状态；清理仅会将原本成功的运行升级为 1；HUP/INT/TERM 分别映射为 129/130/143。
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
compose_file="$repo_root/infra/compose/compose.yaml"
env_example="$repo_root/infra/compose/.env.example"
roots_example="$repo_root/infra/compose/deployment-roots.example.json"
secret_ignore="$repo_root/infra/compose/secrets/.gitignore"

# 将所有消息参数写入 stderr，并以状态 1 终止入口。
fail() {
  printf 'compose check failed: %s\n' "$*" >&2
  exit 1
}

# 要求文件参数 1 包含字面量参数 2；成功时不输出，缺失标记时交由 fail 处理。
require_fixed() {
  file=$1
  text=$2
  grep -F -- "$text" "$file" >/dev/null || fail "$file must contain: $text"
}

for file in "$compose_file" "$env_example" "$roots_example" "$secret_ignore"; do
  [ -f "$file" ] || fail "$file is missing"
done

require_fixed "$compose_file" '${MEDIAFLOW_IMAGE:-mediaflow:0.2.0-m2}'
require_fixed "$compose_file" 'read_only: true'
require_fixed "$compose_file" 'no-new-privileges:true'
require_fixed "$compose_file" 'cap_drop:'
require_fixed "$compose_file" 'user: "${PUID:-1000}:${PGID:-1000}"'
require_fixed "$compose_file" 'MEDIAFLOW_BOOTSTRAP_SECRET_FILE: /run/secrets/mediaflow-bootstrap'
require_fixed "$compose_file" ':/run/secrets/mediaflow-bootstrap:ro'
require_fixed "$compose_file" '/data/incoming:ro'
require_fixed "$compose_file" '/config'
if grep -E '^[[:space:]]+(secrets|uid|gid|mode):' "$compose_file" >/dev/null; then
  fail "$compose_file must use the host-owned read-only secret bind, not ignored file-secret metadata"
fi
compose_runtime_gate=$(sed -n '/^# Docker-backed Compose gate$/,$p' "$0")
for marker in 'test_uid=12345' 'test_gid=23456' 'root-user.json' 'nonnumeric-user.json' 'stat -c %a /run/secrets/mediaflow-bootstrap'; do
  printf '%s\n' "$compose_runtime_gate" | grep -F -- "$marker" >/dev/null \
    || fail "Docker-backed Compose gate must verify: $marker"
done
cleanup_gate=$(sed -n '/^# Root-owned fixture restoration$/,/^# End root-owned fixture restoration$/p' "$0")
for marker in 'host_uid=$(id -u)' 'host_gid=$(id -g)' 'original_status=$?' 'cleanup_error=0' 'trap - EXIT HUP INT TERM' 'chown -R "$1:$2" /fixture' 'chmod -R u+rwX /fixture' 'on_signal()'; do
  printf '%s\n' "$cleanup_gate" | grep -F -- "$marker" >/dev/null \
    || fail "cleanup gate must preserve host removability and exit status: $marker"
done

expected_ignore=$(printf '*\n!.gitignore')
actual_ignore=$(cat "$secret_ignore")
[ "$actual_ignore" = "$expected_ignore" ] || fail "$secret_ignore must contain only the secret deny rule and its exception"

command -v python3 >/dev/null 2>&1 || fail "python3 is required for static JSON validation"
python3 - "$roots_example" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    value = json.load(source)

expected = {
    "roots": [
        {
            "id": "incoming",
            "label": "下载完成",
            "container_path": "/data/incoming",
            "access": "read-only",
        }
    ]
}
if value != expected:
    raise SystemExit("deployment-roots.example.json does not match the locked capability-root shape")
PY

[ "${1:-}" = "--static-only" ] && exit 0

# Docker-backed Compose gate
# 以下为由 Docker 支撑的 Compose 检查；上一行是自检脚本读取的稳定边界标记。
command -v docker >/dev/null 2>&1 || fail "docker command is required for the Compose runtime gate"
docker info >/dev/null 2>&1 || fail "docker daemon is required for the Compose runtime gate"
docker compose version >/dev/null 2>&1 || fail "Docker Compose v2 is required"

# `TMPDIR` 可选地指定创建一次性 Compose 验证夹具的位置；默认使用 `/tmp`。
config_root=$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-compose-check.XXXXXX")
project_name="mediaflow-config-check-$$"
test_uid=12345
test_gid=23456
# `MEDIAFLOW_TEST_IMAGE` 可选地选择用于所有权和运行时检查的不可变本地镜像。
test_image=${MEDIAFLOW_TEST_IMAGE:-mediaflow:0.2.0-m2}
# Root-owned fixture restoration
# 以下恢复测试夹具的宿主所有权；上一行是自检脚本读取的稳定边界标记。
host_uid=$(id -u)
host_gid=$(id -g)
fixture_restore_required=0
compose_cleanup_required=0

# 清除环境中的部署覆盖后，以参数 1 作为 env 文件、其余参数作为 Compose 命令运行 Docker Compose。
# 传播命令 stdout/stderr 与状态；运行时子命令可创建、检查或删除隔离项目。
compose_with_env() {
  compose_env_file=$1
  shift
  env \
    -u MEDIAFLOW_IMAGE \
    -u PUID \
    -u PGID \
    -u MEDIAFLOW_BIND_ADDRESS \
    -u MEDIAFLOW_PORT \
    -u MEDIAFLOW_CONFIG_PATH \
    -u MEDIAFLOW_INCOMING_PATH \
    -u MEDIAFLOW_ROOTS_CONFIG_PATH \
    -u MEDIAFLOW_BOOTSTRAP_SECRET_PATH \
    -u MEDIAFLOW_PUBLIC_ORIGIN \
    -u MEDIAFLOW_TRUSTED_PROXY_CIDRS \
    -u TZ \
    -u COMPOSE_FILE \
    -u COMPOSE_PROJECT_NAME \
    docker compose \
      --project-name "$project_name" \
      --env-file "$compose_env_file" \
      --file "$compose_file" \
      "$@"
}

# 通过测试镜像将临时夹具递归恢复为调用主机的 UID/GID。
# 无需恢复或恢复成功时返回 0；镜像或 Docker 操作失败时输出清理诊断并返回 1。
restore_fixture_ownership() {
  if [ "$fixture_restore_required" -eq 0 ]; then
    return 0
  fi
  if ! docker image inspect "$test_image" >/dev/null 2>&1; then
    printf 'compose cleanup failed: image %s is unavailable; cannot restore temporary fixture ownership\n' "$test_image" >&2
    return 1
  fi
  if ! docker run --rm \
    --user 0:0 \
    --entrypoint /bin/sh \
    --volume "$config_root:/fixture" \
    "$test_image" \
    -eu -c 'chown -R "$1:$2" /fixture; chmod -R u+rwX /fixture' \
    -- "$host_uid" "$host_gid"
  then
    printf 'compose cleanup failed: could not restore temporary fixture ownership to %s:%s\n' "$host_uid" "$host_gid" >&2
    return 1
  fi
}

# 移除所有隔离的 Compose 资源、恢复主机所有权并删除夹具，同时保留原始状态。
# 清理失败仅会将原本成功的运行升级为状态 1；不会掩盖先前的非零状态。
cleanup() {
  original_status=$?
  trap - EXIT HUP INT TERM
  cleanup_error=0

  if [ "$compose_cleanup_required" -eq 1 ]; then
    if ! docker info >/dev/null 2>&1; then
      printf 'compose cleanup failed: Docker daemon is unavailable; cannot remove project %s\n' "$project_name" >&2
      cleanup_error=1
    elif ! compose_with_env "$config_root/positive.env" down --volumes --remove-orphans >/dev/null; then
      printf 'compose cleanup failed: could not remove project %s\n' "$project_name" >&2
      cleanup_error=1
    fi
  fi

  if ! restore_fixture_ownership; then
    cleanup_error=1
  fi
  if ! rm -rf "$config_root"; then
    printf 'compose cleanup failed: could not remove temporary fixture %s\n' "$config_root" >&2
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

mkdir -p "$config_root/config" "$config_root/incoming" "$config_root/secrets"
cp "$roots_example" "$config_root/deployment-roots.json"
printf 'compose-check-placeholder\n' >"$config_root/secrets/bootstrap.secret"
chmod 600 "$config_root/secrets/bootstrap.secret"

# 在目标参数 3 处写入用于 UID 参数 1、GID 参数 2 的完整隔离 Compose env 文件。
# 使用夹具路径和非密钥测试值覆盖目标；写入失败通过 `set -e` 传播。
write_env() {
  uid=$1
  gid=$2
  destination=$3
  {
    printf 'MEDIAFLOW_IMAGE=%s\n' "$test_image"
    printf 'PUID=%s\n' "$uid"
    printf 'PGID=%s\n' "$gid"
    printf 'MEDIAFLOW_BIND_ADDRESS=127.0.0.1\n'
    printf 'MEDIAFLOW_PORT=39091\n'
    printf 'MEDIAFLOW_CONFIG_PATH=%s\n' "$config_root/config"
    printf 'MEDIAFLOW_INCOMING_PATH=%s\n' "$config_root/incoming"
    printf 'MEDIAFLOW_ROOTS_CONFIG_PATH=%s\n' "$config_root/deployment-roots.json"
    printf 'MEDIAFLOW_BOOTSTRAP_SECRET_PATH=%s\n' "$config_root/secrets/bootstrap.secret"
    printf 'MEDIAFLOW_PUBLIC_ORIGIN=https://mediaflow.example.test\n'
    printf 'MEDIAFLOW_TRUSTED_PROXY_CIDRS=172.16.0.0/12\n'
    printf 'TZ=Asia/Shanghai\n'
  } >"$destination"
}

write_env "$test_uid" "$test_gid" "$config_root/positive.env"
compose_with_env "$config_root/positive.env" config --format json >"$config_root/config.json"

cat >"$config_root/validate.py" <<'PY'
import json
import re
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    model = json.load(source)
expected_uid, expected_gid = sys.argv[2:4]

services = model.get("services", {})
if set(services) != {"mediaflow"}:
    raise SystemExit(f"expected exactly one mediaflow service, got {sorted(services)}")
service = services["mediaflow"]
image = service.get("image", "")
if not image or image == "latest" or image.endswith(":latest"):
    raise SystemExit(f"image tag is not immutable: {image!r}")
if service.get("privileged"):
    raise SystemExit("privileged mode is forbidden")
if service.get("network_mode") == "host":
    raise SystemExit("host networking is forbidden")
if not service.get("read_only"):
    raise SystemExit("read_only filesystem is required")
if service.get("restart") != "unless-stopped":
    raise SystemExit("restart policy must be unless-stopped")
user = str(service.get("user", ""))
match = re.fullmatch(r"([0-9]+):([0-9]+)", user)
if match is None or int(match.group(1)) == 0 or int(match.group(2)) == 0:
    raise SystemExit(f"numeric non-root UID:GID is required, got {user!r}")
if match.groups() != (expected_uid, expected_gid):
    raise SystemExit(f"runtime UID:GID does not match requested values: {user!r}")
if "ALL" not in service.get("cap_drop", []):
    raise SystemExit("all Linux capabilities must be dropped")
if not any(str(value).replace(" ", "") == "no-new-privileges:true" for value in service.get("security_opt", [])):
    raise SystemExit("no-new-privileges:true is required")

tmpfs = service.get("tmpfs", [])
if not any(str(value).split(":", 1)[0] == "/tmp" for value in tmpfs):
    raise SystemExit("/tmp tmpfs is required")

volumes = service.get("volumes", [])
by_target = {volume.get("target"): volume for volume in volumes}
for target in ("/config", "/data/incoming", "/run/mediaflow/deployment-roots.json", "/run/secrets/mediaflow-bootstrap"):
    if target not in by_target:
        raise SystemExit(f"missing explicit mount target {target}")
if by_target["/config"].get("read_only"):
    raise SystemExit("/config must be writable")
if not by_target["/data/incoming"].get("read_only"):
    raise SystemExit("incoming capability root must be read-only")
if not by_target["/run/mediaflow/deployment-roots.json"].get("read_only"):
    raise SystemExit("deployment roots configuration must be read-only")
if not by_target["/run/secrets/mediaflow-bootstrap"].get("read_only"):
    raise SystemExit("bootstrap secret bind must be read-only")
for volume in volumes:
    source = str(volume.get("source", ""))
    if source == "/" or "docker.sock" in source:
        raise SystemExit(f"forbidden mount source: {source}")

environment = service.get("environment", {})
if environment.get("MEDIAFLOW_BOOTSTRAP_SECRET_FILE") != "/run/secrets/mediaflow-bootstrap":
    raise SystemExit("bootstrap secret must be supplied through the _FILE variable")
if environment.get("MEDIAFLOW_BOOTSTRAP_SECRET"):
    raise SystemExit("raw bootstrap secret environment value is forbidden")
if service.get("secrets") or model.get("secrets"):
    raise SystemExit("file-backed Compose secret metadata must not be used as a permission assertion")
PY

python3 "$config_root/validate.py" "$config_root/config.json" "$test_uid" "$test_gid"

write_env 0 "$test_gid" "$config_root/root.env"
compose_with_env "$config_root/root.env" config --format json >"$config_root/root-user.json"
if python3 "$config_root/validate.py" "$config_root/root-user.json" 0 "$test_gid" >/dev/null 2>&1; then
  fail "root PUID must be rejected"
fi

write_env not-a-uid "$test_gid" "$config_root/nonnumeric.env"
compose_with_env "$config_root/nonnumeric.env" config --format json >"$config_root/nonnumeric-user.json"
if python3 "$config_root/validate.py" "$config_root/nonnumeric-user.json" not-a-uid "$test_gid" >/dev/null 2>&1; then
  fail "non-numeric PUID must be rejected"
fi

docker image inspect "$test_image" >/dev/null 2>&1 \
  || fail "image $test_image is required; run just check-containers first"

fixture_restore_required=1
docker run --rm \
  --user 0:0 \
  --entrypoint /bin/sh \
  --volume "$config_root:/fixture" \
  "$test_image" \
  -eu -c 'chown "$1:$2" /fixture/config /fixture/secrets/bootstrap.secret; chmod 0700 /fixture/config; chmod 0600 /fixture/secrets/bootstrap.secret' \
  -- "$test_uid" "$test_gid"

compose_cleanup_required=1
compose_with_env "$config_root/positive.env" run --rm --no-deps --entrypoint /bin/sh mediaflow \
  -eu -c '
    test "$(id -u)" = "$1"
    test "$(id -g)" = "$2"
    test -r /run/secrets/mediaflow-bootstrap
    test "$(stat -c %u:%g /run/secrets/mediaflow-bootstrap)" = "$1:$2"
    test "$(stat -c %a /run/secrets/mediaflow-bootstrap)" = 600
    if printf x >>/run/secrets/mediaflow-bootstrap 2>/dev/null; then
      printf "bootstrap secret bind is writable\n" >&2
      exit 1
    fi
  ' -- "$test_uid" "$test_gid"
