# Solidity tests

Run `bash scripts/check-solidity.sh` from the repository root with Foundry 1.8.3.
`forge test` runs only this Solidity suite. The small `Vm` interface in
`TestBase.sol` exposes Foundry cheatcodes without a third-party test dependency.

- `Controllers.t.sol`: namespace and parent authority, exact joint consent,
  expiry/revocation, replay prevention, closure/recreation and contract controllers.
- `Accounting.t.sol`: gross double-entry balances, depth bounds, immutable
  hierarchy, shared-ancestor cancellation, closure and fuzzed conservation.
- `Custody.t.sol`: claim permissions, namespace vault isolation, solvency,
  exact sender/recipient deltas, token failure and reentrancy rollback.
- `Reference.t.sol`: all 162 hierarchy and 138 custody actions in the shared
  fixtures, including every observed rejection and post-action balance. Group
  aggregates, gross supply, collateral and conservation are checked separately.

Fixture reads are the only enabled filesystem permission. The source fixtures
and their commit pin are read directly; no copies or edited expectations are used.
`tests/reference/` remains the separate generator for the pinned baseline.

`TestToken` and `ControllerApp` are test fixtures and are not deployment targets.
