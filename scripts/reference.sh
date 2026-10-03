#!/usr/bin/env bash
# Reproduce the pinned accounting/custody fixtures without editing their source.
set -euo pipefail
cd "$(dirname "$0")/.."
project_dir="$PWD"
contracts_dir="${CAVALRE_CONTRACTS_DIR:-$project_dir/reference/cavalre-contracts}"
deps_dir="${CAVALRE_REFERENCE_DEPS_DIR:-$project_dir/target/reference-deps/node_modules}"

[[ "$(forge --version | head -n 1)" == 'forge Version: 1.8.3' ]] || {
  echo 'Fixture generation is pinned to Foundry 1.8.3.' >&2; exit 1;
}
python3 scripts/verify-reference.py "$contracts_dir"
if [[ ! -d "$deps_dir/forge-std/src" ]]; then
  [[ -z "${CAVALRE_REFERENCE_DEPS_DIR:-}" ]] || {
    echo 'CAVALRE_REFERENCE_DEPS_DIR must contain the installed reference dependencies.' >&2; exit 1;
  }
  mkdir -p target/reference-deps
  cp "$contracts_dir/package.json" "$contracts_dir/package-lock.json" target/reference-deps/
  GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=url.https://github.com/.insteadOf \
    GIT_CONFIG_VALUE_0=ssh://git@github.com/ npm ci --ignore-scripts --prefix target/reference-deps
fi
forge test --root tests/reference --match-test 'testExport.*Fixture' \
  --remappings "contracts/=$contracts_dir/" \
  --remappings "@cavalre/contracts/=$contracts_dir/" \
  --remappings "@openzeppelin/contracts/=$deps_dir/@openzeppelin/contracts/" \
  --remappings "@openzeppelin/contracts-upgradeable/=$deps_dir/@openzeppelin/contracts-upgradeable/" \
  --remappings "forge-std/=$deps_dir/forge-std/" \
  --remappings "solady/=$deps_dir/solady/"
python3 - <<'PY'
import json
from pathlib import Path
for name in ('custody', 'hierarchy'):
    generated = json.loads(Path(f'target/{name}-reference.json').read_text())
    committed = json.loads(Path(f'spec/fixtures/{name}.json').read_text())
    if generated != committed:
        raise SystemExit(f'{name} fixture changed; review before updating the baseline')
    print(f'{name}: {len(generated["steps"])} steps reproduced exactly')
PY
