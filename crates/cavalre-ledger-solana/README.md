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
allocates rent-funded storage and moves native SOL, classic SPL Token or
Token-2022 assets. Custodian,
lifecycle, admission, posting, backing and exact-settlement rules live in the
core. Its `atomic` contract uses Solana transaction rollback: every error is
propagated directly to the entry point. The host is consumed by each call.

The 512-byte `Record` field layout and existing ledger PDA derivation are
preserved. Ledger records now use the canonical global Root as their parent.
The earlier draft's zero-parent records are rejected; use fresh initialization
for this draft. No deployed-state migration or deployment is included.

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

## Global Root

One global `Root` is the parent of every token and accounting ledger. It is a
registered debit group at depth 1 using the existing 512-byte `Record`, including
its ordinary `children` count. `global_root_address()` derives it from `["Root"]`.
Root's parent, relative identity and custodian point to itself. Token ledgers
remain debit groups at depth 2, each with its own credit Source; posting stops
there and Root's balances stay zero.

The first successful ledger creation initializes Root automatically. The payer
receives no global authority. Creating a ledger sets its parent to Root and
increments Root's child count through the same core storage operations used
for other groups. Root, ledger, Source and any vault initialization commit
together. Failed or duplicate creation changes none of them.

`add_ledger` uses `RegisterLedger`; `RegisterToken` and `RegisterSol` also include
a writable `global_root` account. Supply the writable Source PDA as a remaining
account. There is no index-entry account or client-supplied insertion index.
Creations serialize on Root and use its current count at execution. Ordinary
account mutations, transfers and settlement do not read or write Root.

Ledger discovery delegates to Root's child queries. As with other child queries,
the reader validates a complete snapshot of immediate children against the
parent's count, then returns children in address order. Count alone needs only
Root. See [read semantics](../../docs/READS.md). The separate registry format from
the preceding draft is removed; this draft uses fresh initialization and does
not migrate deployed accounts.

## Instructions

- `add_ledger`: create an accounting-only root and Source.
- `add_native_sol`: permissionlessly create the shared SOL root, Source and vault.
- `add_external_token`: permissionlessly create a supported mint's root,
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
- `wrap_sol`, `unwrap_sol`: apply the same funding and withdrawal rules to native
  lamports using `MoveSol` accounts.

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
when listing registered children. `LedgerAdded`, leaf/group creation and removal,
`Credit` and `Debit` report the original structural and posting semantics through
Anchor logs. They replace the draft `AccountChanged` snapshots. Only consume
events from successful transactions; see [event fields and compatibility](../../docs/EVENTS.md).

## Native SOL

`NATIVE_SOL` is the zero public key, also the System Program address. It cannot
be a token mint. SOL has one root `["ledger", zero, NATIVE_SOL]`, one protected
Source, and a System-owned, empty-data vault at `["vault", root]`. Only Ledger
can sign for that vault PDA. The root reports `TokenKind::Native`; existing
Ledger record layouts are unchanged.

`add_native_sol` initializes the fixed name `SOL` and funds the vault to the
runtime's rent-exempt minimum for a zero-data account. An already-funded vault
is accepted. Any excess funding is surplus custody and creates no claim.
Root/Source storage funding and vault rent are separate from customer balances.
Repeated registration fails without changing existing state.

`wrap_sol` and `unwrap_sol` take `parent`, `relative` and a `u64` lamport amount.
Supply `payer`, `authority`, `funding_authority`, `root`, `vault`, `wallet` and
`system_program`, followed by the usual Ledger account paths:

- On deposit, `wallet` is the funding address and `funding_authority` must sign
  for that same address. System Program transfer requires a System-owned source
  with no data. The branch authority independently authorizes the receiver.
- On withdrawal, `wallet` is the selected writable recipient and needs no
  signature. `funding_authority` may reuse the branch authority. The vault signs
  its System transfer using Ledger's PDA seeds.
- The fee/rent payer may also be the funding wallet or payout recipient. Exact
  settlement is measured around the SOL transfer; later account allocation and
  transaction fees do not become customer claims.

Available backing is `vault.lamports - Rent::minimum_balance(0)`. Withdrawals
check that amount against all outstanding SOL claims and preserve the rent
reserve, including when the last customer withdraws. Deposits must observe a
fresh wallet debit and matching vault increase; donations cannot fund them.
Ledger uses raw lamports (1 SOL = 1e9 lamports), with `u128` internal balances.
The recipient is subject to normal runtime account/rent rules.

Native SOL uses System transfers directly. Wrapped SOL mint ledgers, if admitted
through the token interface, remain separate assets; there is no automatic
conversion or shared accounting between them. See the executable
[native SOL examples](../../tests/runtime/native_sol.rs).

## Token compatibility

The existing instructions accept classic SPL Token and Token-2022 through
Anchor's shared token interface. Supply the mint's owning token program in
`token_program`; the mint, vault and wallet must all belong to that program.
The root remains unique per mint, with the same Source, custody permissions,
base-unit amounts and exact-settlement checks. No discretionary mint list is used.

Token-2022 extensions are inspected at registration and on every deposit and
withdrawal. The current policy is:

| Extension | Treatment |
| --- | --- |
| No extensions | Supported |
| `MetadataPointer`, `TokenMetadata` | Supported; descriptive data does not change raw balances |
| `GroupPointer`, `TokenGroup`, `GroupMemberPointer`, `TokenGroupMember` | Supported; group metadata does not change settlement |
| `MintCloseAuthority` | Supported; the token program enforces its supply checks |
| `InterestBearingConfig`, `ScaledUiAmount` | Supported in raw base units; Ledger does not apply UI multipliers or accrue additional units |
| Token-account `ImmutableOwner` | Supported, including Token-2022 associated accounts |
| All other mint or token-account extensions | Rejected with `UnsupportedToken`; malformed data also rejects |

The rejected set includes transfer fees, transfer hooks, permanent delegates,
confidential transfers, nontransferable/pausable tokens, default account states,
CPI guards and memo requirements. This version has no settlement mechanism for
those configurations. Fee and hook extensions are rejected even when their
current fee is zero or their hook is disabled. Unknown extension data cannot
silently become supported. Custody compatibility is independent of symbol
availability. `LedgerView` reads native SOL metadata, mint decimals, Metaplex
symbols and Token-2022 inline symbols; see [read interfaces](../../docs/READS.md)
for source validation, metadata pointers and undefined-field behavior.

Both token programs retain their own mint/freeze authority behavior. Runtime
settlement failures roll back token movement, accounting and new allocations.
Existing token instruction arguments, account order and stored record layout
are unchanged; clients select the appropriate token program.

References: [Anchor token interface](https://www.anchor-lang.com/docs/tokens/basics/transfer-tokens)
and [Token-2022 extensions](https://solana.com/docs/tokens/extensions).

## Supported scope and release status

- Native SOL, classic SPL Token and the Token-2022 configurations above are supported.
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
