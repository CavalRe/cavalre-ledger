# Working on CavalRe Ledger

- Work on `main` and push completed, verified changes when authorized.
- Shared rules live in `crates/cavalre-ledger-core`; Solana-specific behavior
  lives in `crates/cavalre-ledger-solana`. Follow the original Solidity LedgerLib
  baseline and keep recognizable module names and responsibilities.
- The core must remain reusable, `no_std` and free of platform SDK dependencies.
  Hosts own address derivation, storage, signer verification and token movement.
  Do not duplicate accounting rules in adapters or name downstream consumers.
- Preserve implicit leaves, effective flags, relative identities, custody
  ancestry, explicit Source and the original double-entry ancestor walk.
- Use Solana-native storage and signer validation without importing the previous
  per-account controller model or separate application token trees.
- Keep SR, Multiswap and consensus policy in consumers. Standalone Solidity is
  deferred until the Solana implementation settles.
- Treat `reference/cavalre-contracts` and pinned fixtures as read-only comparison
  material. Never weaken expectations to make a port pass or include the
  reference checkout as an implementation dependency.
- Keep one authoritative balance store. Authenticate account ownership, identity
  and signatures; never trust client-supplied flags or account snapshots.
- Distinguish storage allocation from logical registration. Preserve atomic
  accounting and custody settlement, checked arithmetic and backing checks.
- Run `bash scripts/check.sh` for active Rust changes. Runtime tests must use freshly built sBPF, including the test consumer.
  `scripts/build-sbf.sh` enforces the pinned build and stack-limit checks.
- Preserve pinned Rust, Agave and platform-tools versions. Run
  `bash scripts/reference.sh` for reference-affecting changes.
- See `docs/PORTING.md` before recovering old plumbing or tests. Adapt them to the
  original model rather than restoring the superseded architecture.
- Generated output belongs under `target/`. Keep credentials and private
  operational material out of this public repository.
- Do not publish crates, deploy programs, create signing keys or change upgrade
  authority without explicit authorization.
