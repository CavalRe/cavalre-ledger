# Solidity contracts

This directory is the home for the standalone EVM implementation of CavalRe
Ledgers. The repository layout and Foundry configuration are ready; the EVM
contracts and deployment architecture have not yet been implemented or selected.

Use the [accounting model](../docs/ACCOUNTING.md),
[portable service rules](../docs/LEDGER_CORE.md), and shared fixtures in
[`spec/fixtures/`](../spec/fixtures/) as the behavioral reference. EVM storage,
authorization and custody adapters must enforce those rules using EVM-native
mechanisms.

The root `foundry.toml` uses Solidity 0.8.26 and Cancun, matching the pinned
Solidity reference. Run Foundry from the repository root; tests belong in
[`tests/solidity/`](../tests/solidity/), and generated output stays under
`target/solidity/`.

The pinned `reference/cavalre-contracts` checkout and `tests/reference` fixture
generator remain separate, read-only specification inputs. They are not sources
or dependencies of this Foundry project.
