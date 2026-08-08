#!/usr/bin/env bash
set -euo pipefail

required=(
  MEDIAFLOW_AUTOMATION_LIVE_ROOT
  MEDIAFLOW_AUTOMATION_LIVE_SENTINEL
  MEDIAFLOW_AUTOMATION_LIVE_RSS_URL
  MEDIAFLOW_AUTOMATION_LIVE_OLLAMA_BASE_URL
  MEDIAFLOW_AUTOMATION_LIVE_OLLAMA_MODEL
  MEDIAFLOW_DOWNLOAD_TEST_SOURCE
  MEDIAFLOW_QBITTORRENT_BASE_URL
  MEDIAFLOW_QBITTORRENT_USERNAME
  MEDIAFLOW_QBITTORRENT_PASSWORD
  MEDIAFLOW_TRANSMISSION_BASE_URL
  MEDIAFLOW_TRANSMISSION_USERNAME
  MEDIAFLOW_TRANSMISSION_PASSWORD
)
missing=()
for name in "${required[@]}"; do
  if [[ -z "${!name:-}" ]]; then missing+=("$name"); fi
done
if (( ${#missing[@]} > 0 )); then
  printf 'M4 source automation live acceptance: SKIPPED (missing %s)\n' "${missing[*]}"
  printf 'M4 source automation live acceptance: DEFERRED (real RSS, signed Webhook/completion bridge, downloaders, Ollama, NAS root, and fault exercise were not jointly verified)\n'
  exit 0
fi
if [[ "$MEDIAFLOW_AUTOMATION_LIVE_SENTINEL" != "I_ACKNOWLEDGE_ISOLATED_AUTOMATION_ROOT" ]]; then
  printf 'M4 source automation live acceptance: SKIPPED (acknowledgement sentinel mismatch)\n'
  exit 0
fi

canonical_directory() {
  local value="$1"
  [[ -d "$value" ]] || return 1
  (cd -- "$value" && pwd -P)
}

overlaps() {
  local first="$1" second="$2"
  [[ "$first" == "$second" || "$first" == "$second/"* || "$second" == "$first/"* ]]
}

base_root="$(canonical_directory "$MEDIAFLOW_AUTOMATION_LIVE_ROOT")" || {
  printf 'M4 source automation live acceptance: SKIPPED (live root is not an existing directory)\n'
  exit 0
}
project_root="$(cd -- "$(dirname -- "$0")/../../.." && pwd -P)"
workspace_root="$(git -C "$project_root" rev-parse --show-toplevel)"
config_root="${MEDIAFLOW_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/mediaflow}"
if [[ -d "$config_root" ]]; then config_root="$(canonical_directory "$config_root")"; fi
if [[ "$base_root" == "/" || "$base_root" == "$HOME" || "$base_root" == "$workspace_root" || "$base_root" == "$project_root" || "$base_root" == "$config_root" ]]; then
  printf 'M4 source automation live acceptance: SKIPPED (unsafe live root)\n'
  exit 0
fi
if overlaps "$base_root" "$workspace_root" || overlaps "$base_root" "$project_root" || overlaps "$base_root" "$config_root"; then
  printf 'M4 source automation live acceptance: SKIPPED (live root overlaps workspace or config)\n'
  exit 0
fi
if [[ ! -r "$base_root" || ! -w "$base_root" || ! -x "$base_root" ]]; then
  printf 'M4 source automation live acceptance: SKIPPED (live root lacks isolated read/write/search access)\n'
  exit 0
fi

run_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
test_root="$base_root/.mediaflow-source-automation-live-$run_id"
marker='mediaflow-source-automation-live-owned'
cleanup() {
  local status="$?"
  trap - EXIT HUP INT TERM
  case "$(basename -- "$test_root")" in .mediaflow-source-automation-live-*) ;; *) exit "$status" ;; esac
  if [[ -f "$test_root/.mediaflow-live-owned" ]] && [[ "$(sed -n '1p' "$test_root/.mediaflow-live-owned")" == "$marker" ]]; then
    rm -rf -- "$test_root"
  fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

mkdir -- "$test_root"
mkdir -- "$test_root/incoming"
printf '%s\n' "$marker" > "$test_root/.mediaflow-live-owned"
printf 'mediaflow live acceptance fixture\n' > "$test_root/incoming/Arrival.2016.mkv"
export MEDIAFLOW_AUTOMATION_LIVE_TEST_ROOT="$test_root"

printf 'M4 source automation live acceptance: RUNNING (paths, sources, endpoints, credentials, and prompts are redacted)\n'
cargo test --manifest-path apps/core/Cargo.toml --test automation_live \
  real_feed_ollama_and_capability_root_obey_the_production_boundaries -- --ignored --exact --nocapture
cargo test --manifest-path apps/core/Cargo.toml --test downloaders_live -- --ignored --nocapture
cargo test --manifest-path apps/core/Cargo.toml \
  --test automation_webhook --test automation_webhook_security \
  --test download_completion_automation
printf 'M4 source automation live acceptance: PASSED (real RSS/Ollama/downloader protocols, isolated capability root, signed Webhook, and completion bridge; created downloader tasks were not deleted)\n'
