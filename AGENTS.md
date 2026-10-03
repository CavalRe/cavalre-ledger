# Working on CavalRe Ledgers

- Work on `main` and push completed, verified changes when authorized.
- This is the standalone Ledger product: accounting kernel, portable service,
  and Solana adapter. Keep SR, Multiswap and consensus policy in consumers.
- Rust lives in `crates/`; Solidity lives in `contracts/`, with tests in
  `tests/solidity/` and root Foundry configuration. Keep language-specific
  tooling independent and generated output under `target/`.
- The root Foundry project is separate from `tests/reference/`. Never include
  the pinned reference checkout as an implementation dependency.
- Read `docs/LEDGER_CORE.md`, `docs/ACCOUNTING.md` and
  `docs/OMNIBUS_LEDGER.md` before changing economics or authority boundaries.
- Treat `reference/cavalre-contracts` and pinned fixtures as read-only
  specifications. Never weaken expectations to make a port pass.
- Keep one authoritative balance store. Core types are views of host state.
  Preserve gross debit/credit accounting and cancellation at shared ancestors.
- Hosts authenticate record identity, ownership and signatures. Never accept
  client-supplied snapshots or unverified signer IDs as authoritative state.
- Namespace and parent controllers have structural rights, not inherited
  permission to spend descendants. Preserve endpoint consent and root types.
- Preserve atomic commit, checked amount bounds, exact custody deltas and the
  full-liability solvency check before withdrawals. Donations issue no claims.
- Keep the core deterministic, `no_std` and free of chain SDK dependencies.
- Keep existing Solana account layouts, instruction encoding and token support
  policy unless an explicit migration is designed and tested.
- Run `bash scripts/check.sh`; integration tests must execute freshly compiled
  sBPF. Run `bash scripts/reference.sh` for reference-affecting changes.
- Preserve the pinned Rust, Agave and platform-tools versions. Stack-limit
  diagnostics are build failures even if the compiler exits successfully.
- Test consumers and simulation identities are not deployment targets.
- This repository is public. Keep credentials and private operational material
  out of source control.
- Do not publish crates, deploy programs, introduce signing keys or change
  upgrade authority without explicit user authorization.
