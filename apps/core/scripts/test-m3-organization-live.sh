#!/usr/bin/env bash
set -euo pipefail

required=(
  MEDIAFLOW_ORGANIZATION_LIVE_SOURCE_ROOT
  MEDIAFLOW_ORGANIZATION_LIVE_TARGET_ROOT
  MEDIAFLOW_ORGANIZATION_LIVE_SENTINEL
)
missing=()
for name in "${required[@]}"; do
  if [[ -z "${!name:-}" ]]; then missing+=("$name"); fi
done
if (( ${#missing[@]} > 0 )); then
  printf 'M3 organization live acceptance: SKIPPED (missing %s)\n' "${missing[*]}"
  exit 0
fi
if [[ "$MEDIAFLOW_ORGANIZATION_LIVE_SENTINEL" != "I_ACKNOWLEDGE_ISOLATED_TEST_ROOTS" ]]; then
  printf 'M3 organization live acceptance: SKIPPED (acknowledgement sentinel mismatch)\n'
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

device_id() {
  if stat -f '%d' "$1" >/dev/null 2>&1; then
    stat -f '%d' "$1"
  else
    stat -c '%d' "$1"
  fi
}

source_root="$(canonical_directory "$MEDIAFLOW_ORGANIZATION_LIVE_SOURCE_ROOT")" || {
  printf 'M3 organization live acceptance: SKIPPED (source root is not an existing directory)\n'
  exit 0
}
target_root="$(canonical_directory "$MEDIAFLOW_ORGANIZATION_LIVE_TARGET_ROOT")" || {
  printf 'M3 organization live acceptance: SKIPPED (target root is not an existing directory)\n'
  exit 0
}
project_root="$(cd -- "$(dirname -- "$0")/../../.." && pwd -P)"
workspace_root="$(git -C "$project_root" rev-parse --show-toplevel)"
config_root="${MEDIAFLOW_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/mediaflow}"
if [[ -d "$config_root" ]]; then config_root="$(canonical_directory "$config_root")"; fi

for root in "$source_root" "$target_root"; do
  if [[ "$root" == "/" || "$root" == "$HOME" || "$root" == "$workspace_root" || "$root" == "$project_root" || "$root" == "$config_root" ]]; then
    printf 'M3 organization live acceptance: SKIPPED (unsafe root target)\n'
    exit 0
  fi
  if overlaps "$root" "$workspace_root" || overlaps "$root" "$project_root" || overlaps "$root" "$config_root"; then
    printf 'M3 organization live acceptance: SKIPPED (root overlaps workspace or config)\n'
    exit 0
  fi
  if [[ ! -r "$root" || ! -w "$root" || ! -x "$root" ]]; then
    printf 'M3 organization live acceptance: SKIPPED (root lacks isolated read/write/search access)\n'
    exit 0
  fi
done
if overlaps "$source_root" "$target_root"; then
  printf 'M3 organization live acceptance: SKIPPED (source and target roots overlap)\n'
  exit 0
fi

source_device="$(device_id "$source_root")"
target_device="$(device_id "$target_root")"
if [[ "$source_device" != "$target_device" ]]; then
  printf 'M3 organization live acceptance: SKIPPED (base roots must share a device for hardlink coverage)\n'
  exit 0
fi

run_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
source_test_root="$source_root/.mediaflow-organization-live-$run_id"
target_test_root="$target_root/.mediaflow-organization-live-$run_id"
cross_test_root=""
marker='mediaflow-organization-live-owned'

cleanup() {
  local status="$?"
  trap - EXIT HUP INT TERM
  for owned in "$cross_test_root" "$target_test_root" "$source_test_root"; do
    [[ -n "$owned" ]] || continue
    case "$(basename -- "$owned")" in .mediaflow-organization-live-*) ;; *) continue ;; esac
    if [[ -f "$owned/.mediaflow-live-owned" ]] && [[ "$(sed -n '1p' "$owned/.mediaflow-live-owned")" == "$marker" ]]; then
      rm -rf -- "$owned"
    fi
  done
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

mkdir -- "$source_test_root" "$target_test_root"
printf '%s\n' "$marker" > "$source_test_root/.mediaflow-live-owned"
printf '%s\n' "$marker" > "$source_test_root/.outside-operation-sentinel"
printf '%s\n' "$marker" > "$target_test_root/.mediaflow-live-owned"
printf '%s\n' "$marker" > "$target_test_root/.outside-operation-sentinel"
export MEDIAFLOW_ORGANIZATION_LIVE_SOURCE_TEST_ROOT="$source_test_root"
export MEDIAFLOW_ORGANIZATION_LIVE_TARGET_TEST_ROOT="$target_test_root"
unset MEDIAFLOW_ORGANIZATION_LIVE_CROSS_DEVICE_TEST_ROOT

cross_status="SKIPPED (not configured)"
if [[ -n "${MEDIAFLOW_ORGANIZATION_LIVE_CROSS_DEVICE_TARGET_ROOT:-}" ]]; then
  cross_root="$(canonical_directory "$MEDIAFLOW_ORGANIZATION_LIVE_CROSS_DEVICE_TARGET_ROOT")" || {
    printf 'M3 organization live acceptance: SKIPPED (optional cross-device root is invalid)\n'
    exit 0
  }
  if [[ "$cross_root" == "/" || "$cross_root" == "$HOME" ]] || overlaps "$cross_root" "$source_root" || overlaps "$cross_root" "$target_root" || overlaps "$cross_root" "$workspace_root" || overlaps "$cross_root" "$config_root"; then
    printf 'M3 organization live acceptance: SKIPPED (optional cross-device root is unsafe or overlapping)\n'
    exit 0
  fi
  if [[ ! -r "$cross_root" || ! -w "$cross_root" || ! -x "$cross_root" ]]; then
    printf 'M3 organization live acceptance: SKIPPED (optional cross-device root lacks access)\n'
    exit 0
  fi
  cross_device="$(device_id "$cross_root")"
  if [[ "$cross_device" == "$source_device" ]]; then
    cross_status="SKIPPED (configured root is on the base device)"
  else
    cross_test_root="$cross_root/.mediaflow-organization-live-$run_id"
    mkdir -- "$cross_test_root"
    printf '%s\n' "$marker" > "$cross_test_root/.mediaflow-live-owned"
    printf '%s\n' "$marker" > "$cross_test_root/.outside-operation-sentinel"
    export MEDIAFLOW_ORGANIZATION_LIVE_CROSS_DEVICE_TEST_ROOT="$cross_test_root"
    cross_status="PASSED (different device observed)"
  fi
fi

printf 'M3 organization live acceptance: RUNNING (isolated paths redacted; base device=%s)\n' "$source_device"
cargo test --manifest-path apps/core/Cargo.toml --test organization_live \
  isolated_roots_cover_recovery_catalog_operations_and_safe_rollback -- --ignored --exact --nocapture
printf 'M3 organization live acceptance: PASSED (movie/episode/generic operations, restart, Catalog, NFO preserve and rollback); cross-device=%s\n' "$cross_status"
