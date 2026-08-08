#!/usr/bin/env bash
set -euo pipefail

required=(
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
  printf 'M4 downloader live acceptance: SKIPPED (missing %s)\n' "${missing[*]}"
  exit 0
fi

printf 'M4 downloader live acceptance: RUNNING (sources and credentials are redacted)\n'
cargo test --manifest-path apps/core/Cargo.toml --test downloaders_live \
  -- --ignored --nocapture
printf 'M4 downloader live acceptance: PASSED (created tasks were observed; nothing was deleted)\n'
