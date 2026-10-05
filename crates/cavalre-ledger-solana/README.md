# Solana Ledger

Solana program using the reusable `cavalre-ledger-core`, based on original
LedgerLib commit `34d159ff4e88fdfdee16738d9a1228f0bf407212`.

| Solidity | Rust |
| --- | --- |
| `LedgerLib.sol` | Core `ledger_lib.rs`, with Solana address derivation in the adapter |
| `Ledger.sol` | Core `ledger.rs` operations; Solana host in `ledger.rs` and entry points in `lib.rs` |
| `LedgerView.sol` | `ledger_view.rs` |

Read clients can depend on this crate with `default-features = false`. That
excludes the mutation program and SPL call dependency. Shared `Record` decoding
and PDA derivation live in `ledger_lib.rs`; `ledger_view::Reader` queries a
validated account snapshot without invoking Ledger. Existing exports from
`ledger` remain available when mutations are enabled. See [read usage](../../docs/READS.md).

## Accounts and authority

Each entry point invokes the shared core service through `SolanaHost`. The host
authenticates runtime signers, validates and serializes accounts, derives PDAs,
allocates rent-funded storage and performs standard SPL movement. Custodian,
lifecycle, admission, posting, backing and exact-settlement rules live in the
core. Its `atomic` contract uses Solana transaction rollback: every error is
propagated directly to the entry point. The host is consumed by each call.

This extraction retains the existing instruction arguments and 512-byte `Record`
layout; it does not require a data migration.

The host decodes each supplied record once and borrows its metadata for core
reads. It updates balances and child counts directly and tracks changed records
without a second before/after snapshot. Solana owns rollback; the core still
computes the same checked posting changes before applying them.

An external mint has one root PDA: `["ledger", zero public key, mint]`.
All applications share that tree and its vault `["vault", root]`. Each root has
an explicit credit Source at `["account", root, SOURCE]`; `SOURCE` is the
exported fixed relative identifier. All other external-token accounts are debits.

An accounting-only root uses `["ledger", authority, identifier]`. Its signed
authority manages its accounts and authorizes postings, including credit issuance.
The identifier is an application-chosen public key, not a required token mint.
Such balances have no native redemption route. This is the initial root-ownership
interface; there is no delegation or recovery mechanism yet.

A child PDA is `["account", absolute parent, relative identifier]`. Direct
external-token children require that relative identifier to sign when created.
Descendants require the branch's direct-child authority. An application uses a
PDA signer through CPI; its administrator wallet cannot substitute for that PDA.
Tree operations use runtime signatures, not off-chain intents.

`Record` stores identity, flags, custody ancestry, current `u128` debit and
credit balances, registration, parent admission, child count and name. Metadata
and balances share a 512-byte account; logical registration is independent of
storage allocation. First receipt allocates the destination with the transaction's
rent payer, without requiring the recipient's signature or registration.
The allocating payer receives no balance authority.

Groups choose whether immediate implicit children are allowed. This policy is
fixed while the group is registered. Matching repeated registration is a no-op;
conflicting metadata and registration over incompatible balances are rejected.
Removal requires zero gross balances and no registered children. Removal clears
registration, retaining storage; rent reclamation is not implemented.

## Instructions

- `add_ledger`: create an accounting-only root and Source.
- `add_external_token`: permissionlessly create a standard SPL mint's root,
  Source and vault. The initializer acquires no special spending authority.
- `add_sub_account`, `add_sub_account_group`: register accounts with custodian
  authorization and existing balance/kind checks.
- `remove_sub_account`, `remove_sub_account_group`: unregister empty accounts;
  authorized removal of an implicit leaf is a no-op.
- `transfer`: same-custodian debit transfers for external tokens; authorized
  debit/credit postings for accounting-only ledgers.
- `wrap`: the branch authority selects a receiver, and the token payer signs.
  Transfer tokens into the vault, verify exact deltas, credit Source and debit
  the receiver atomically.
- `unwrap`: the branch authority selects a funded debit leaf and recipient token
  account. Check backing for all claims, credit the leaf, debit Source and pay
  the recipient atomically. Recipient registration/signature is unnecessary.

`MoveTokens.funding_authority` signs for the depositing wallet. On withdrawal,
it may be the already-required branch authority; no recipient signature is needed.
The amount is `u64` native base units. Internal transfer amounts are `u128`.
No rescaling occurs.

Supply all endpoint records, parents, custody ancestors and changed ancestors as
remaining accounts, once each, excluding the fixed root. Include new destination
and Source PDAs for allocation. Records whose data changes must be writable.
The program verifies owners, canonical PDAs and root membership. See executable
instruction-building examples in `tests/runtime/ledger.rs`.

Clients can deserialize public `Record` data after its eight-byte `CVLEDG01`
header. Parent is at byte offset 40 for RPC filtering; filter registered records
when listing registered children. `AccountChanged` reports updated balances and
registration after mutations; this new schema is not ERC20 event compatibility.

## Supported scope and release status

- Standard SPL Token only. Token-2022, transfer hooks, transfer-fee tokens and
  direct native SOL are not supported by this first executable implementation.
- Cross-custodian external-token transfers, off-chain intents, delegated app
  authority, upgrades/migration policy and rent reclamation are not implemented.
- Group cancellation preserves the original accounting walk; the current
  instruction contexts conservatively lock the root writable. Parallel account
  scheduling and removal of unnecessary root locks remain optimization work.
- No resource-based depth cap is imposed. Depth remains a checked `u8`, with
  root depth 2. Applications must budget their complete transactions, including
  other instructions and CPI calls. See [execution measurements](../../docs/EXECUTION_LIMITS.md)
  for compute, signed packet sizes, rent and client construction guidance.
- Names are bounded to 64 bytes. External mint decimals come from the mint.
  Internal units are raw integers; presentation precision is application policy.
- No deployed service, production program identity or upgrade authority has been
  selected. The checked-in ID and test-consumer identities are simulation-only.
- Tests establish the exercised behavior; they do not establish full spec parity
  or constitute a security audit. Do not treat this as a mainnet release merely
  because it builds.

Run `bash scripts/check.sh` with the pinned Rust and Agave tools on PATH. It
builds actual sBPF before running runtime tests; there is no native substitute.
