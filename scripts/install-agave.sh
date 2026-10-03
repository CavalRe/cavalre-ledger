#!/usr/bin/env bash
# Pinned Linux/x86_64 development toolchain; no deployment or wallet creation.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -sm)" == 'Linux x86_64' ]] || {
  echo 'This installer supports Linux x86_64. Install Agave 4.3.0 for your platform.' >&2
  exit 1
}
mkdir -p target/toolchains
archive="$PWD/target/toolchains/agave-v4.3.0.tar.bz2"
curl --fail --location --retry 3 \
  https://github.com/anza-xyz/agave/releases/download/v4.3.0/solana-release-x86_64-unknown-linux-gnu.tar.bz2 \
  --output "$archive"
# Digest published on the official Agave v4.3.0 release asset.
printf '%s  %s\n' c97289a8abb1d0efb497d8b5cb285baabd9b7f8ea6647f5d145c5dc8ff3611e8 "$archive" | sha256sum --check
tar --extract --bzip2 --no-same-owner --file "$archive" --directory target/toolchains
printf 'Add to PATH: %s/target/toolchains/solana-release/bin\n' "$PWD"
