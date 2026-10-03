#!/usr/bin/env bash
# Pinned Linux/x86_64 Solidity development tools.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -sm)" == 'Linux x86_64' ]] || {
  echo 'This installer supports Linux x86_64. Install Foundry 1.8.3 for your platform.' >&2
  exit 1
}
mkdir -p target/toolchains/foundry
archive="$PWD/target/toolchains/foundry-v1.8.3.tar.gz"
curl --fail --location --retry 3 \
  https://github.com/foundry-rs/foundry/releases/download/v1.8.3/foundry_v1.8.3_linux_amd64.tar.gz \
  --output "$archive"
# SHA-256 published on the official Foundry v1.8.3 release asset.
printf '%s  %s\n' 7ca48e6ca3cac1bce1403ca67e5bc1dc3bc1fd818199c9957c7165079c228568 "$archive" | sha256sum --check
tar --extract --gzip --no-same-owner --file "$archive" --directory target/toolchains/foundry
printf 'Add to PATH: %s/target/toolchains/foundry\n' "$PWD"
