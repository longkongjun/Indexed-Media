#!/usr/bin/env bash
# Keep native tool status and full diagnostics independent of terminal summaries.
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "$0")/../.." && pwd)
output_root=$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-ios-check.XXXXXX")
printf 'iOS 检查报告：%s\n' "$output_root"
cd "$repo_root"

run_check() {
    local name=$1 result
    shift
    if "$@" >"$output_root/$name.log" 2>&1; then
        result=0
    else
        result=$?
    fi
    printf '%s\t%s\n' "$name" "$result" >>"$output_root/status.tsv"
    tail -20 "$output_root/$name.log"
    return "$result"
}

run_check swiftlint just client-lint-ios
cd "$repo_root/apps/ios"
run_check xcodegen xcodegen generate
run_check xcodebuild xcodebuild test -project MediaFlow.xcodeproj -scheme MediaFlowiOS \
    -destination "${MEDIAFLOW_IOS_DESTINATION:-platform=iOS Simulator,name=iPhone 17}" \
    -resultBundlePath "$output_root/tests.xcresult"
