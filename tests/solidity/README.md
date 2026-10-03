# Solidity tests

Place the standalone EVM implementation's Foundry tests here, using the `.t.sol`
suffix. Run `forge test` from the repository root with Foundry 1.8.3.

No Solidity implementation tests exist yet. As the contracts are implemented,
replay the shared fixtures in `spec/fixtures/` and cover authorization, hierarchy,
exact custody settlement and atomic rollback. Any fixture read permissions or
test dependencies should be added explicitly when needed.

`tests/reference/` is the separate generator for the pinned Solidity baseline;
it is not included in this test suite.
