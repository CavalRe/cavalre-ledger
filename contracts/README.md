# Solidity contracts

| Contract | Responsibility |
| --- | --- |
| `Ledgers.sol` | Namespaces, immutable account topology, controller consent, journal postings and ERC20 claim accounting |
| `LedgerVault.sol` | One immutable ERC20 vault per namespace/token; only its Ledgers service can release collateral |

Read the [EVM interface](../docs/LEDGER_EVM.md) for operations, authorization,
examples, limitations and verification. The [accounting model](../docs/ACCOUNTING.md)
and [portable service rules](../docs/LEDGER_CORE.md) are the behavioral reference.

Use root `foundry.toml` and `bash scripts/check-solidity.sh`. Solidity 0.8.26
targets Cancun; tests live under `tests/solidity/`, and generated files under
`target/solidity/`. The implementation has no third-party Solidity dependencies.

`reference/cavalre-contracts` is a pinned, read-only specification checkout,
separate from this implementation and its build. This port does not migrate
existing accounts or replace the original Dispatcher module in place.
