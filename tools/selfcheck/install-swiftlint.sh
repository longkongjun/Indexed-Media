#!/usr/bin/env bash
# Install a checksum-verified official binary in an explicit local directory.
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "$0")/../.." && pwd)
source "$repo_root/tools/selfcheck/tool-versions.env"
if [[ $# != 1 || -z $1 || -e $1 ]]; then
    printf 'usage: %s <new-local-directory>\n' "$0" >&2
    exit 2
fi
mkdir -p "$1"
output_root=$(cd -- "$1" && pwd)
curl --fail --location --retry 2 \
    "https://github.com/realm/SwiftLint/releases/download/$SWIFTLINT_VERSION/portable_swiftlint.zip" \
    -o "$output_root/swiftlint.zip"
printf '%s  %s\n' "$SWIFTLINT_SHA256" "$output_root/swiftlint.zip" | shasum -a 256 -c -
unzip -q "$output_root/swiftlint.zip" -d "$output_root"
test "$("$output_root/swiftlint" version)" = "$SWIFTLINT_VERSION"
