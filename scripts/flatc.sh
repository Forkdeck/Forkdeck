#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
dir="$root/.local/flatc"
archive_sha=de0c6ad114a5a686ecf64322528c602c7d4512446a93f290f54f00ee5abea487
binary_sha=9f495eb01661b28ae2c4a499c1e6272ca1265d250cbd86cd267cb5685c4fadad
if ! echo "$binary_sha  $dir/flatc" | sha256sum --check --status 2>/dev/null; then
    mkdir -p "$dir"
    curl -fL 'https://github.com/google/flatbuffers/releases/download/v25.9.23/Linux.flatc.binary.g%2B%2B-13.zip' -o "$dir/flatc.zip"
    echo "$archive_sha  $dir/flatc.zip" | sha256sum --check >&2
    unzip -o "$dir/flatc.zip" -d "$dir" >&2
    echo "$binary_sha  $dir/flatc" | sha256sum --check >&2
fi
printf '%s\n' "$dir/flatc"
