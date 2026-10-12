#!/bin/sh
# media-capability.v1 网关的 Compose 冒烟：模拟一个 AI 调用方（mock DSH）走完整协议链路。
# 链路：令牌认证边界 → 只读能力调用 → 幂等重放 → 事件 cursor 拉取 → 管理员投影的认证边界。
# 输入：可选 `--static-only`；运行时需要 `MEDIAFLOW_TEST_IMAGE`（或默认 m2 镜像）与 Docker。
# 副作用：创建临时 Compose 项目与夹具目录；退出时全部清理。
# 退出：成功 0；验证/运行时/清理失败 1。
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
compose_file="$repo_root/infra/compose/compose.yaml"

fail() { printf 'capability smoke failed: %s\n' "$*" >&2; exit 1; }
require_marker() { grep -F -- "$1" "$0" >/dev/null || fail "missing capability smoke marker: $1"; }

if [ "${1:-}" = "--static-only" ]; then
  for marker in \
    'MEDIAFLOW_CAPABILITY_TOKEN' '/api/capability/v1/calls' '/api/capability/v1/events' \
    '/api/v1/capability-events' 'Bearer wrong-token' 'capability.completed' \
    'docker compose up -d' 'trap - EXIT HUP INT TERM'; do
    require_marker "$marker"
  done
  printf 'capability smoke static markers: OK (runtime not executed)\n'
  exit 0
fi

[ -f "$compose_file" ] || fail "$compose_file is missing"
command -v docker >/dev/null 2>&1 || fail "docker command is required"
docker info >/dev/null 2>&1 || fail "Docker daemon is unavailable"
command -v python3 >/dev/null 2>&1 || fail "python3 is required for JSON assertions"

test_image=${MEDIAFLOW_TEST_IMAGE:-mediaflow:0.3.0}
case "$test_image" in latest|*:latest) fail "test image must use immutable non-latest tag: $test_image" ;; esac
docker image inspect "$test_image" >/dev/null 2>&1 || fail "test image $test_image is unavailable; run just check-containers first"

fixture_root=
cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ -n "$fixture_root" ]; then
    if [ -f "$fixture_root/compose.yaml" ]; then
      (cd "$fixture_root" && docker compose -p "$project_name" down -v --remove-orphans >/dev/null 2>&1) || status=1
    fi
    rm -rf -- "$fixture_root" || status=1
  fi
  exit "$status"
}
on_signal() { exit "$1"; }
trap cleanup EXIT
trap 'on_signal 129' HUP
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

fixture_root=$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-capability-smoke.XXXXXX")
project_name="mediaflow-capability-smoke-$$"
config_root="$fixture_root/config"
incoming_root="$fixture_root/incoming"
secret_root="$fixture_root/secrets"
mkdir -p "$config_root" "$incoming_root" "$secret_root"
printf 'bootstrap-secret-for-capability-smoke-0001\n' > "$secret_root/bootstrap"
chmod 600 "$secret_root/bootstrap"
printf '{"roots":[{"id":"incoming","label":"Incoming","access":"read-only","container_path":"/data/incoming"}]}\n' \
  > "$fixture_root/deployment-roots.json"

capability_token="capability-smoke-token-0123456789abcdef"
cat > "$fixture_root/smoke.env" <<ENV
MEDIAFLOW_TEST_IMAGE=$test_image
MEDIAFLOW_PUBLIC_ORIGIN=https://mediaflow-capability-smoke.example.test
MEDIAFLOW_TRUSTED_PROXY_CIDRS=127.0.0.1/32
MEDIAFLOW_PORT=32167
MEDIAFLOW_BIND_ADDRESS=127.0.0.1
MEDIAFLOW_CONFIG_PATH=$config_root
MEDIAFLOW_INCOMING_PATH=$incoming_root
MEDIAFLOW_ROOTS_CONFIG_PATH=$fixture_root/deployment-roots.json
MEDIAFLOW_BOOTSTRAP_SECRET_PATH=$secret_root/bootstrap
MEDIAFLOW_CAPABILITY_TOKEN=$capability_token
PUID=$(id -u)
PGID=$(id -g)
ENV

(cd "$fixture_root" && docker compose -p "$project_name" -f "$compose_file" --env-file smoke.env up -d >/dev/null) \
  || fail "compose up failed"

base_url="http://127.0.0.1:32167"
ready=0
i=0
while [ "$i" -lt 60 ]; do
  if curl -fsS "$base_url/health/ready" >/dev/null 2>&1; then ready=1; break; fi
  i=$((i + 1)); sleep 1
done
[ "$ready" = "1" ] || fail "service did not become ready within 60s"

# mock DSH 第一步：错误令牌必须被稳定拒绝（permission）。
wrong_token_status=$(curl -s -o "$fixture_root/wrong.json" -w '%{http_code}' \
  -X POST "$base_url/api/capability/v1/calls" \
  -H "Authorization: Bearer wrong-token-value-0000000000" \
  -H "Content-Type: application/json" \
  -d '{"protocol_version":"media-capability.v1","request_id":"019f8a20-0000-7000-8000-000000000001","session_id":null,"task_id":null,"capability":"media.task-status","actor":{"kind":"ai","id":"dsh-smoke"},"input":{},"execution":{"mode":"read-only","idempotency_key":"smoke-wrong-01","expected_revision":null,"deadline_ms":30000}}')
python3 - "$fixture_root/wrong.json" "$wrong_token_status" <<'PY_ASSERT' || fail "wrong-token rejection must be structured permission error"
import json, sys
body = json.load(open(sys.argv[1]))
assert sys.argv[2] == "200", f"expected 200 envelope, got {sys.argv[2]}"
assert body["status"] == "rejected"
assert body["error"]["category"] == "permission", body["error"]
PY_ASSERT

# mock DSH 第二步：正确令牌的合法调用被接受；任务不存在走结构化 failed（task not found）。
call_body=$(cat <<JSON
{"protocol_version":"media-capability.v1","request_id":"019f8a20-0000-7000-8000-000000000002","session_id":null,"task_id":null,"capability":"media.task-status","actor":{"kind":"ai","id":"dsh-smoke"},"input":{"task_id":"019f8a20-0000-7000-8000-0000000000ff"},"execution":{"mode":"read-only","idempotency_key":"smoke-status-01","expected_revision":null,"deadline_ms":30000}}
JSON
)
curl -fsS -o "$fixture_root/status.json" -X POST "$base_url/api/capability/v1/calls" \
  -H "Authorization: Bearer $capability_token" \
  -H "Content-Type: application/json" -d "$call_body" || fail "authenticated call failed"
python3 - "$fixture_root/status.json" <<'PY_ASSERT' || fail "missing-task call must fail with structured not_found"
import json, sys
body = json.load(open(sys.argv[1]))
assert body["status"] == "failed", body
assert body["error"]["category"] == "not_found", body["error"]
PY_ASSERT

# mock DSH 第三步：同一 request_id 重放返回缓存结果（幂等）。
curl -fsS -o "$fixture_root/replay.json" -X POST "$base_url/api/capability/v1/calls" \
  -H "Authorization: Bearer $capability_token" \
  -H "Content-Type: application/json" -d "$call_body" || fail "replay call failed"
cmp -s "$fixture_root/status.json" "$fixture_root/replay.json" || fail "replay must return the cached result verbatim"

# mock DSH 第四步：事件 cursor 拉取返回已留痕的能力事件。
curl -fsS -o "$fixture_root/events.json" "$base_url/api/capability/v1/events?limit=10" \
  -H "Authorization: Bearer $capability_token" || fail "event pull failed"
python3 - "$fixture_root/events.json" <<'PY_ASSERT' || fail "capability events must replay by cursor"
import json, sys
body = json.load(open(sys.argv[1]))
items = body["items"]
assert len(items) >= 1, "expected at least one persisted capability event"
sequences = [item["sequence"] for item in items]
assert sequences == sorted(sequences), "events must be sequence-ordered"
PY_ASSERT

# 管理员投影必须要求会话认证（capability 令牌不可换用为管理员凭据）。
admin_status=$(curl -s -o /dev/null -w '%{http_code}' "$base_url/api/v1/capability-events" \
  -H "Authorization: Bearer $capability_token")
[ "$admin_status" = "401" ] || fail "admin projection must reject capability tokens (got $admin_status)"

printf 'capability compose smoke: OK\n'
