#!/bin/sh
# 执行静态自检，或仅对夹具具有破坏性的 Compose 冷启动、API 持久化、重启与清理冒烟测试。
# 输入：可选 `--static-only`；`MEDIAFLOW_TEST_IMAGE`、`MEDIAFLOW_SMOKE_UID`、`MEDIAFLOW_SMOKE_GID` 和 `TMPDIR` 可覆盖测试输入。
# 输出/副作用：输出摘要；运行时模式会创建临时 Compose 项目、本地夹具文件和引导/会话数据。
# 退出：成功时为 0，验证/运行时/清理失败时为 1；无效 UID/GID 会失败；HUP/INT/TERM 分别映射为 129/130/143。
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
compose_file="$repo_root/infra/compose/compose.yaml"
roots_example="$repo_root/infra/compose/deployment-roots.example.json"

# 将所有消息参数写入 stderr，并以状态 1 终止冒烟测试入口。
fail() { printf 'compose smoke failed: %s\n' "$*" >&2; exit 1; }
# 要求本脚本包含字面量参数 1，使静态检查证明对应运行时断言仍存在。
require_marker() { grep -F -- "$1" "$0" >/dev/null || fail "missing smoke marker: $1"; }

if [ "${1:-}" = "--static-only" ]; then
  command -v python3 >/dev/null 2>&1 || fail "python3 is required for static smoke validation"
  for marker in \
    'docker info' 'docker compose version' 'docker image inspect' \
    'docker compose config --format json' 'docker compose up -d' \
    'docker compose restart mediaflow' 'MEDIAFLOW_TEST_IMAGE:-mediaflow:0.2.0-m2' \
    'read_only' '/data/incoming:ro' '/run/mediaflow/deployment-roots.json:ro' \
    '/run/secrets/mediaflow-bootstrap:ro' '/proc/1/cmdline' \
    'trap - EXIT HUP INT TERM' 'original_status=$?' 'cleanup_error=0' \
    'chown -R "$1:$2" /fixture' 'chmod -R u+rwX /fixture' 'on_signal()'; do
    require_marker "$marker"
  done
  # 受保护的 GET 必须显式接收并转发会话 Cookie；Secure Cookie 不会由 HTTP 测试客户端自动发送。
  api_helper=$(sed -n '/^cat >"\$fixture_root\/api.py"/,/^PY_API$/p' "$0")
  printf '%s\n' "$api_helper" | grep -F -- 'def get(path, headers=None):' >/dev/null \
    || fail 'protected GET helper must accept explicit authentication headers'
  printf 'compose smoke static markers: OK (runtime not executed)\n'
  exit 0
fi

[ -f "$compose_file" ] || fail "$compose_file is missing"
[ -f "$roots_example" ] || fail "$roots_example is missing"
command -v docker >/dev/null 2>&1 || fail "docker command is required; install Docker Engine/Desktop with Compose v2"
docker info >/dev/null 2>&1 || fail "Docker daemon is unavailable; start Docker before running smoke-m2-compose"
docker compose version >/dev/null 2>&1 || fail "Docker Compose v2 is required (docker compose version failed)"
command -v python3 >/dev/null 2>&1 || fail "python3 is required for JSON and HTTP assertions"

# `MEDIAFLOW_TEST_IMAGE` 可选地选择冒烟测试执行的不可变预构建镜像。
test_image=${MEDIAFLOW_TEST_IMAGE:-mediaflow:0.2.0-m2}
case "$test_image" in latest|*:latest) fail "test image must use immutable non-latest tag: $test_image" ;; esac
docker image inspect "$test_image" >/dev/null 2>&1 || fail "test image $test_image is unavailable; run just check-containers first"

# 在 mktemp/初始化前安装最小陷阱，避免早期失败或信号期间泄漏夹具。
fixture_root=
# 删除部分创建的夹具，同时保留入口处捕获的命令状态。
early_cleanup() {
  early_status=$?
  trap - EXIT HUP INT TERM
  if [ -n "$fixture_root" ]; then rm -rf -- "$fixture_root" || early_status=1; fi
  exit "$early_status"
}
# 将数值信号状态参数 1 转换为进程状态，使早期 EXIT 清理陷阱可运行。
early_signal() { early_signal_status=$1; trap - HUP INT TERM; exit "$early_signal_status"; }
trap early_cleanup EXIT
trap 'early_signal 129' HUP
trap 'early_signal 130' INT
trap 'early_signal 143' TERM

# `TMPDIR` 可选地指定创建一次性运行时夹具的位置；默认使用 `/tmp`。
fixture_root=$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-m2-compose-smoke.XXXXXX")
project_name="mediaflow-m2-smoke-$$"
config_root="$fixture_root/config"
incoming_root="$fixture_root/incoming"
secret_root="$fixture_root/secrets"
env_file="$fixture_root/smoke.env"
config_json="$fixture_root/compose-config.json"
logs_file="$fixture_root/compose.log"
fact_file="$fixture_root/fact.json"
host_uid=$(id -u)
host_gid=$(id -g)
# `MEDIAFLOW_SMOKE_UID` 与 `MEDIAFLOW_SMOKE_GID` 可选地为夹具选择数值型非 root 容器所有权。
test_uid=${MEDIAFLOW_SMOKE_UID:-$host_uid}
test_gid=${MEDIAFLOW_SMOKE_GID:-$host_gid}

compose_cleanup_required=0
fixture_ownership_changed=0
# 清除环境中的部署覆盖后，使用生成的 env 文件为隔离项目运行 Docker Compose。
# 所有函数参数均转发给 Compose；传播 stdout/stderr、状态和 Docker 副作用。
compose_with_env() {
  env -u MEDIAFLOW_IMAGE -u PUID -u PGID -u MEDIAFLOW_BIND_ADDRESS -u MEDIAFLOW_PORT \
    -u MEDIAFLOW_CONFIG_PATH -u MEDIAFLOW_INCOMING_PATH -u MEDIAFLOW_ROOTS_CONFIG_PATH \
    -u MEDIAFLOW_BOOTSTRAP_SECRET_PATH -u MEDIAFLOW_PUBLIC_ORIGIN -u MEDIAFLOW_TRUSTED_PROXY_CIDRS \
    -u TZ -u COMPOSE_PROJECT_NAME docker compose --project-name "$project_name" --env-file "$env_file" \
    --file "$compose_file" "$@"
}
# 通过测试镜像将运行时夹具递归恢复为调用主机的 UID/GID。
# 无变化或恢复成功时返回 0；无法恢复时输出清理诊断并返回 1。
restore_fixture_ownership() {
  [ "$fixture_ownership_changed" -eq 0 ] && return 0
  docker image inspect "$test_image" >/dev/null 2>&1 || { printf 'compose cleanup failed: image unavailable\n' >&2; return 1; }
  docker run --rm --user 0:0 --entrypoint /bin/sh --volume "$fixture_root:/fixture" "$test_image" \
    -eu -c 'chown -R "$1:$2" /fixture; chmod -R u+rwX /fixture' -- "$host_uid" "$host_gid" || {
      printf 'compose cleanup failed: could not restore fixture ownership to %s:%s\n' "$host_uid" "$host_gid" >&2
      return 1
    }
}
# 项目需要清理时尽力将 Compose 日志捕获到夹具中；始终返回 0。
capture_logs() { [ "$compose_cleanup_required" -eq 1 ] && compose_with_env logs --no-color >"$logs_file" 2>&1 || true; }
# 捕获日志、移除 Compose 项目、恢复所有权并删除夹具，同时保留原始状态。
# 清理失败仅会将原本成功的运行升级为状态 1，且不会隐藏先前失败。
cleanup() {
  original_status=$?
  trap - EXIT HUP INT TERM
  cleanup_error=0
  capture_logs
  if [ "$compose_cleanup_required" -eq 1 ]; then
    if ! docker info >/dev/null 2>&1; then
      printf 'compose cleanup failed: Docker daemon unavailable; project %s remains\n' "$project_name" >&2
      cleanup_error=1
    elif ! compose_with_env down --volumes --remove-orphans >/dev/null 2>&1; then
      printf 'compose cleanup failed: could not remove project %s\n' "$project_name" >&2
      cleanup_error=1
    fi
  fi
  restore_fixture_ownership || cleanup_error=1
  rm -rf -- "$fixture_root" || cleanup_error=1
  if [ "$original_status" -eq 0 ] && [ "$cleanup_error" -ne 0 ]; then original_status=1; fi
  exit "$original_status"
}
# 将数值信号状态参数 1 转换为进程状态，使主 EXIT 清理陷阱运行一次。
on_signal() { signal_status=$1; trap - HUP INT TERM; exit "$signal_status"; }
trap cleanup EXIT
trap 'on_signal 129' HUP
trap 'on_signal 130' INT
trap 'on_signal 143' TERM
case "$test_uid" in ''|*[!0-9]*) fail "MEDIAFLOW_SMOKE_UID must be numeric" ;; esac
case "$test_gid" in ''|*[!0-9]*) fail "MEDIAFLOW_SMOKE_GID must be numeric" ;; esac
if [ "$test_uid" -eq 0 ] || [ "$test_gid" -eq 0 ]; then test_uid=10001; test_gid=10001; fi

mkdir -p "$config_root" "$incoming_root" "$secret_root"
printf 'mediaflow-compose-smoke-%s\n' "$$" >"$incoming_root/m2-smoke.txt"
cp "$roots_example" "$fixture_root/deployment-roots.json"
printf 'mediaflow-smoke-bootstrap-secret-%s\n' "$$" >"$secret_root/bootstrap.secret"
chmod 0700 "$config_root"; chmod 0755 "$incoming_root"; chmod 0600 "$secret_root/bootstrap.secret"; chmod 0644 "$fixture_root/deployment-roots.json"
port=$(python3 - <<'PY_PORT'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY_PORT
)
{
  printf 'MEDIAFLOW_IMAGE=%s\n' "$test_image"
  printf 'PUID=%s\n' "$test_uid"
  printf 'PGID=%s\n' "$test_gid"
  printf 'MEDIAFLOW_BIND_ADDRESS=127.0.0.1\nMEDIAFLOW_PORT=%s\n' "$port"
  printf 'MEDIAFLOW_CONFIG_PATH=%s\nMEDIAFLOW_INCOMING_PATH=%s\n' "$config_root" "$incoming_root"
  printf 'MEDIAFLOW_ROOTS_CONFIG_PATH=%s\nMEDIAFLOW_BOOTSTRAP_SECRET_PATH=%s\n' "$fixture_root/deployment-roots.json" "$secret_root/bootstrap.secret"
  printf 'MEDIAFLOW_PUBLIC_ORIGIN=https://mediaflow.example.test\nMEDIAFLOW_TRUSTED_PROXY_CIDRS=172.16.0.0/12\nTZ=Asia/Shanghai\n'
} >"$env_file"

compose_cleanup_required=1
compose_with_env config --format json >"$config_json"
python3 - "$config_json" "$test_uid" "$test_gid" <<'PY_VALIDATE'
import json, re, sys
model = json.load(open(sys.argv[1], encoding="utf-8"))
service = model.get("services", {}).get("mediaflow")
if set(model.get("services", {})) != {"mediaflow"} or service is None: raise SystemExit("expected exactly one mediaflow service")
image = str(service.get("image", ""))
if not image or image == "latest" or image.endswith(":latest"): raise SystemExit("image tag must be immutable")
if service.get("privileged") or service.get("network_mode") == "host": raise SystemExit("privileged/host network forbidden")
if not service.get("read_only"): raise SystemExit("read_only filesystem required")
if str(service.get("user", "")) != f"{sys.argv[2]}:{sys.argv[3]}" or re.fullmatch(r"[1-9][0-9]*:[1-9][0-9]*", str(service.get("user", ""))) is None: raise SystemExit("numeric non-root user required")
if "ALL" not in service.get("cap_drop", []): raise SystemExit("cap_drop ALL required")
if not any(str(v).replace(" ", "") == "no-new-privileges:true" for v in service.get("security_opt", [])): raise SystemExit("no-new-privileges required")
volumes = service.get("volumes", [])
by_target = {v.get("target"): v for v in volumes if isinstance(v, dict)}
for target in ("/config", "/data/incoming", "/run/mediaflow/deployment-roots.json", "/run/secrets/mediaflow-bootstrap"):
    if target not in by_target: raise SystemExit(f"missing mount {target}")
if by_target["/config"].get("read_only"): raise SystemExit("/config must be writable")
for target in ("/data/incoming", "/run/mediaflow/deployment-roots.json", "/run/secrets/mediaflow-bootstrap"):
    if not by_target[target].get("read_only"): raise SystemExit(f"mount {target} must be read-only")
for volume in volumes:
    source = str(volume.get("source", ""))
    if source == "/" or "docker.sock" in source: raise SystemExit(f"forbidden mount source {source}")
environment = service.get("environment", {})
if isinstance(environment, list): environment = {item.split("=", 1)[0]: item.split("=", 1)[1] for item in environment if "=" in item}
if environment.get("MEDIAFLOW_BOOTSTRAP_SECRET_FILE") != "/run/secrets/mediaflow-bootstrap": raise SystemExit("_FILE secret mount required")
if environment.get("MEDIAFLOW_BOOTSTRAP_SECRET"): raise SystemExit("raw bootstrap secret env forbidden")
if service.get("secrets") or model.get("secrets"): raise SystemExit("Compose secret metadata forbidden")
PY_VALIDATE

fixture_ownership_changed=1
docker run --rm --user 0:0 --entrypoint /bin/sh --volume "$fixture_root:/fixture" "$test_image" \
  -eu -c 'chown -R "$1:$2" /fixture/config /fixture/secrets; chmod 0700 /fixture/config; chmod 0600 /fixture/secrets/bootstrap.secret' -- "$test_uid" "$test_gid"
compose_with_env up -d
container_id=$(compose_with_env ps -q mediaflow)
[ -n "$container_id" ] || fail "Compose did not create a mediaflow container"
running_count=$(compose_with_env ps --status running -q | awk 'NF { n++ } END { print n + 0 }')
[ "$running_count" -eq 1 ] || fail "expected one running Compose container, got $running_count"
project_running_count=$(docker ps --filter "label=com.docker.compose.project=$project_name" --format '{{.ID}}' | awk 'NF { n++ } END { print n + 0 }')
[ "$project_running_count" -eq 1 ] || fail "expected one running project container, got $project_running_count"

attempt=0; health_status=starting
while [ "$attempt" -lt 90 ]; do
  health_status=$(docker inspect --format '{{.State.Health.Status}}' "$container_id" 2>/dev/null || true)
  case "$health_status" in healthy) break ;; unhealthy) capture_logs; fail "container unhealthy" ;; esac
  attempt=$((attempt + 1)); sleep 1
done
[ "$health_status" = healthy ] || { capture_logs; fail "container did not become healthy within 90 seconds"; }
runtime_user=$(docker inspect --format '{{.Config.User}}' "$container_id")
[ "$runtime_user" = "$test_uid:$test_gid" ] || fail "container user $runtime_user is not $test_uid:$test_gid"
pid_one=$(docker exec "$container_id" /bin/sh -eu -c 'tr "\000" " " </proc/1/cmdline')
case "$pid_one" in '/usr/local/bin/mediaflow-core serve'|'/usr/local/bin/mediaflow-core serve ') ;; *) fail "PID 1 is not Core: $pid_one" ;; esac
process_table=$(docker top "$container_id" -eo pid,comm)
process_count=$(printf '%s\n' "$process_table" | awk 'NR > 1 { n++ } END { print n + 0 }')
[ "$process_count" -eq 1 ] || fail "expected one process, got $process_count"
[ "$(printf '%s\n' "$process_table" | awk 'NR == 2 { print $2 }')" = mediaflow-core ] || fail "sole process is not mediaflow-core"

python3 - "http://127.0.0.1:$port" <<'PY_HTTP'
import sys
import urllib.request

base = sys.argv[1]
with urllib.request.urlopen(base + "/", timeout=10) as response:
    body = response.read().decode("utf-8")
    if response.status != 200 or response.headers.get_content_type() != "text/html" or '<div id="app">' not in body:
        raise SystemExit("static Web index was not reachable")
PY_HTTP

cat >"$fixture_root/api.py" <<'PY_API'
import http.cookiejar, json, pathlib, sys, time, urllib.error, urllib.request
base, mode, secret_path, fact_path = sys.argv[1:]
secret = pathlib.Path(secret_path).read_text(encoding="utf-8").strip(); fact_file = pathlib.Path(fact_path)
jar = http.cookiejar.CookieJar(); opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
def request(method, path, body=None, headers=None, expected=200):
    values = {"Accept": "application/json", "Origin": "https://mediaflow.example.test"}; values.update(headers or {})
    payload = json.dumps(body).encode() if body is not None else None
    if payload is not None: values["Content-Type"] = "application/json"
    try:
        with opener.open(urllib.request.Request(base + path, data=payload, headers=values, method=method), timeout=10) as response:
            value = json.loads(response.read().decode())
            if response.status != expected: raise SystemExit(f"{method} {path} returned {response.status}, expected {expected}")
            return value
    except urllib.error.HTTPError as error:
        raise SystemExit(f"{method} {path} returned HTTP {error.code}: {error.read().decode(errors='replace')[:300]}")
def get(path, headers=None): return request("GET", path, headers=headers)
def post(path, body=None, headers=None, expected=201): return request("POST", path, body, headers, expected)
status = get("/api/v1/system/bootstrap-status")
if mode == "create":
    if status != {"requires_initialization": True, "version": "v1"}: raise SystemExit(f"unexpected fresh status: {status!r}")
    post("/api/v1/system/bootstrap", {"bootstrap_secret": secret, "administrator_name": "smoke-admin", "password": "m2-compose-smoke-password"})
elif status != {"requires_initialization": False, "version": "v1"}: raise SystemExit(f"bootstrap state did not persist: {status!r}")
session = post("/api/v1/sessions", {"administrator_name": "smoke-admin", "password": "m2-compose-smoke-password"})
csrf = session.get("csrf_token")
if not isinstance(csrf, str) or len(csrf) < 43: raise SystemExit("login did not return CSRF token")
auth = {"Cookie": "; ".join(f"{c.name}={c.value}" for c in jar), "X-CSRF-Token": csrf}
if mode == "create":
    preflight = post("/api/v1/inbox-directories/preflight", {"root_id": "incoming", "relative_path": "."}, auth, 200)
    if not preflight.get("readable") or preflight.get("overlaps_existing"): raise SystemExit(f"incoming preflight unusable: {preflight!r}")
    inbox = post("/api/v1/inbox-directories", {"root_id": "incoming", "relative_path": "."}, auth); inbox_id = inbox["id"]
    task = post(f"/api/v1/inbox-directories/{inbox_id}/scan-tasks", None, {**auth, "Idempotency-Key": "m2-compose-smoke-scan-v1"}, 202); task_id = task["id"]
    deadline = time.time() + 90
    while task.get("status") in {"queued", "running"} and time.time() < deadline: time.sleep(.5); task = get(f"/api/v1/scan-tasks/{task_id}", auth)
    if task.get("status") in {"queued", "running"}: raise SystemExit(f"scan did not finish: {task!r}")
    files = get(f"/api/v1/scan-tasks/{task_id}/files", auth); items = files.get("items", [])
    if len(items) != 1 or items[0].get("relative_path") != "m2-smoke.txt": raise SystemExit(f"one-file observation missing: {files!r}")
    fact_file.write_text(json.dumps({"account_id": session["account"]["id"], "inbox_id": inbox_id, "task_id": task_id, "status": task["status"], "file_id": items[0]["id"]}) + "\n")
else:
    fact = json.loads(fact_file.read_text()); task_id = fact["task_id"]; task = get(f"/api/v1/scan-tasks/{task_id}", auth)
    if session["account"]["id"] != fact["account_id"]: raise SystemExit("persisted account fact changed after restart")
    if task.get("inbox_directory_id") != fact["inbox_id"]: raise SystemExit("persisted inbox fact changed after restart")
    if task.get("id") != task_id or task.get("status") != fact["status"]: raise SystemExit(f"task fact changed after restart: {task!r}")
    page = get("/api/v1/scan-tasks?limit=200", auth); matches = [item for item in page.get("items", []) if item.get("id") == task_id]
    if len(matches) != 1: raise SystemExit(f"task duplicated or lost: {page!r}")
    files = get(f"/api/v1/scan-tasks/{task_id}/files", auth); items = files.get("items", [])
    if len(items) != 1 or items[0].get("id") != fact["file_id"] or items[0].get("relative_path") != "m2-smoke.txt": raise SystemExit(f"file result duplicated or lost: {files!r}")
print(json.dumps({"mode": mode, "task_id": task_id, "status": task["status"], "file_count": len(items)}, sort_keys=True))
PY_API

base_url="http://127.0.0.1:$port"
python3 "$fixture_root/api.py" "$base_url" create "$secret_root/bootstrap.secret" "$fact_file"
compose_with_env restart mediaflow
attempt=0; health_status=starting
while [ "$attempt" -lt 90 ]; do
  health_status=$(docker inspect --format '{{.State.Health.Status}}' "$container_id" 2>/dev/null || true)
  case "$health_status" in healthy) break ;; unhealthy) capture_logs; fail "container unhealthy after restart" ;; esac
  attempt=$((attempt + 1)); sleep 1
done
[ "$health_status" = healthy ] || { capture_logs; fail "container not healthy after restart"; }
python3 "$fixture_root/api.py" "$base_url" verify "$secret_root/bootstrap.secret" "$fact_file"
printf 'compose smoke: cold start, persistent task/result, restart, and cleanup checks passed\n'
