#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(forge --version | head -n 1)" == 'forge Version: 1.8.3' ]] || {
  echo 'Expected Foundry 1.8.3; see README.md.' >&2
  exit 1
}
forge fmt --check contracts tests/solidity
forge build --sizes contracts/Ledgers.sol
forge test
