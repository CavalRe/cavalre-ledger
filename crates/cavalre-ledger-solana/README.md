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

The 512-byte record allocation is unchanged. The
record stores a one-based `sub_index`, symbol and decimals in previously reserved space. Earlier
draft records without these metadata fields and child indexes require fresh initialization;
they are not a supported upgrade target. No deployed-state migration or deployment
is included.

The host decodes each supplied record once and borrows its metadata for core
reads. It updates balances and child counts directly and tracks changed records
without a second before/after snapshot. Solana owns rollback; the core still
computes the same checked posting changes before applying them.

An external token's logical ledger address is its mint address. This is the
identity used in parent links, discovery, events and view/instruction arguments.
It carries no implication about the mint's Solana account owner or signing rights.
`ledger_address(zero, mint)` returns that identity. Its accounting record lives
at `root_storage_address(zero, mint)`, the PDA `["ledger", zero, mint]`.
The `root` account meta in Anchor instructions supplies this **storage PDA**.
All applications share the tree and its vault `["vault", root_storage]`.
The explicit credit Source is `["account", mint, SOURCE]`; `SOURCE` is the
exported fixed relative identifier. All other external-token accounts are debits.

This corrects the earlier draft's use of the root storage PDA as its logical
identity. External/native child and index addresses change because their parent
is now the asset identifier. Old draft records require fresh initialization;
there is no deployed state to migrate. Root storage and custody vault derivations,
record sizes, instruction account counts and permission rules are unchanged.
Rust clients replace the old `root_address` helper with `root_storage_address`
for account metas and `ledger_address` for logical root identifiers. The core's
accounting rules and interfaces are unchanged.

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

`ledger_lib::name_to_address(name)` derives a relative identity from the full
Keccak-256 hash of the name's exact UTF-8 bytes. `to_address_by_name(program,
parent, name)` returns its child PDA and bump using the existing seeds. Names
require 1–64 bytes and are case-sensitive, with no trimming or normalization.
These helpers work without the `mutations` feature and do not allocate storage.
The reserved `SOURCE` identifier is the compile-time hash of `"Source"`, so named
lookup resolves the same protected credit account without a special case.

Named creation delegates to the explicit creation path. It needs the same
accounts, child-index slots and authenticated custodian; a name hash grants no
signing authority. Use named children inside an external token's application
branch, whose direct child remains the application's signer identity. An
accounting-only root's authority can create named children directly. Explicit
identities may share display names, and can repeat named creation idempotently
when the identity, metadata and flags match.

`Record` stores identity, flags, custody ancestry, current `u128` debit and
credit balances, registration, parent admission, child count, reverse index, name,
symbol and decimals. Metadata
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
together. Failed creation changes none of them. Matching repeat creation succeeds without
writes, allocation or events; conflicting metadata rejects.

`add_ledger` uses `RegisterLedger`; `RegisterToken` and `RegisterSol` also include
a writable `global_root` account. The remaining accounts include writable Source,
Root's next child slot, and the new ledger's slot zero for Source. Creation uses
the current Root count; if another creation changes it after accounts are chosen,
the transaction rejects atomically and the client retries with the current slot.
Ordinary account mutations, transfers and settlement do not use global Root.

Ledger discovery delegates to Root's ordinary child index. Count needs only Root;
lookup needs one slot; pagination needs only the requested slots. Order follows
insertion and swap-and-pop removal, matching Solidity. See [read semantics](../../docs/READS.md).

## Child index accounts

Every group uses the same maintained child array. `child_index_address(parent, i)`
derives its zero-based slot PDA from `["subs", parent, i.to_le_bytes()]`. An 80-byte
slot stores the parent, position and optional child identity. Each registered
child record stores its one-based `sub_index`; implicit leaves have zero and do
not occupy slots. Root slots hold absolute ledger addresses; other slots hold
relative identities.

Supply these writable accounts in addition to the existing tree-mutation inputs:

| Operation | Index accounts |
| --- | --- |
| Create ledger | Root slot `Root.children`, ledger slot zero, and Source record |
| Register leaf/group | Parent slot `parent.children`; matching repeat registration needs no slot |
| Remove registered child | Its slot `child.sub_index - 1`, the last slot `parent.children - 1`, and the last child's record if different |
| Remove implicit leaf | None |
| Transfer, deposit, withdrawal | None |

Supply each account once. Removing a child swaps the last entry into its position,
updates that child's reverse index, clears the last slot and decrements the count
atomically. Empty slots retain their rent-funded allocation and are reused on
later appends; this draft does not reclaim their rent. Index storage costs one
80-byte account per allocated child position, in addition to child records.

Counts and reverse indexes must come from a current snapshot. A concurrent tree
mutation can change the required accounts; stale transactions fail atomically
and must be rebuilt. Readers need only their requested slots, not all siblings.

## Instructions

- `add_ledger(id, name, symbol, decimals)`: create an accounting-only root and Source.
- `add_native_sol`: permissionlessly create the shared SOL root, Source and vault.
- `add_external_token()`: read and validate issuer name, symbol and mint decimals,
  then create the supported mint's root, Source and vault with that snapshot.
  Supply required canonical Metaplex metadata as a readonly remaining account;
  inline Token-2022 metadata is read from the mint. No metadata arguments or
  special spending authority are granted to the initializer.
- `add_sub_account`, `add_sub_account_group`: register accounts with custodian
  authorization and existing balance/kind checks.
- `add_sub_account_by_name(parent, name, credit)` and
  `add_sub_account_group_by_name(parent, name, credit, implicit_allowed)`:
  derive the relative identity from the name and use the same registration path.
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

Every external/native mutation first requires custody to cover the root's full
credit total. A deficit rejects transfers, deposits, withdrawals, tree changes
and repeated or zero/self operations with `Undercollateralized`. Reads continue.
Repair uses a direct SPL/Token-2022/System transfer to the canonical vault,
without crediting any Ledger account. Partial repairs remain frozen; full
backing restores operation automatically. Accounting-only ledgers and other
token ledgers remain independent. There is no stored freeze flag or admin bypass.

`MoveTokens.funding_authority` signs for the depositing wallet. On withdrawal,
it may be the already-required branch authority; no recipient signature is needed.
The amount is `u64` native base units. Internal transfer amounts are `u128`.
No rescaling occurs.

`MoveTokens.root` and `MoveSol.root` accept read-only access. For nonzero
wrap/unwrap, the client must explicitly mark the root writable: these postings
cross Source credit and receiver debit and change both root balances. Generated
Anchor account metas default the root to read-only; adjust the instruction's
metas before signing. Existing transactions with writable
roots remain valid. A missing required root write rejects atomically at commit,
including rollback of native token movement and new leaf allocation.

For direct clients and application CPI, enable this crate's `cpi` feature and use
`ledger_cpi::set_custody_root_writable(&mut instruction)`. It checks the encoded
wrap/unwrap instruction's kind and layout, then sets only the root meta from the
encoded amount: writable for nonzero, read-only for zero. It leaves the encoded
data and all other metas intact, with no allocation or re-encoding. Apply it
before signing a direct transaction or invoking Ledger:

```rust,ignore
use anchor_lang::solana_program::program::invoke_signed;
use cavalre_ledger_solana::ledger_cpi::set_custody_root_writable;

// Reuse an encoded Ledger instruction, or construct it once from arguments.
set_custody_root_writable(&mut instruction)?;
invoke_signed(&instruction, account_infos, signer_seeds)?;
```

The outer transaction must still supply the required writable root, Source,
endpoint and ancestors for nonzero settlement. Preparation cannot grant privileges
missing from that transaction; Solana rejects such a call before entering Ledger.
Anchor's generated `cpi::wrap`/`unwrap`/`wrap_sol`/`unwrap_sol` keep the root
read-only, so prepare the instruction and invoke it directly for custody.
The typed `ledger_cpi` wrappers have been removed to avoid their additional call
preparation costs. The [test consumer](../../tests/consumer/src/lib.rs) forwards
encoded instructions using the in-place helper and an application PDA signer.

For zero amounts, all Ledger records can be read-only, including root, Source,
receiver and ancestors. The full authorization, admission, backing and native
transfer validation still runs, with the same posting events and no allocation.
Vault, wallet and payer retain their writable requirements; zero amounts still
invoke the native token/System transfer. These shared writable accounts can
still serialize custody calls.

Supply all endpoint records, parents, custody ancestors and changed ancestors as
remaining accounts, once each, excluding the fixed root. Include new destination
and Source PDAs for allocation. Records whose data changes must be writable.
The program verifies owners, canonical PDAs and root membership. See executable
instruction-building examples in `tests/runtime/ledger.rs`.

For external/native `LedgerAccounts` calls, also supply the canonical vault
`["vault", root_storage]` as a **read-only remaining account**. The adapter
authenticates its address, owner and layout; token vaults must match the ledger
mint and root storage authority. The core compares its actual balance with all
claims before executing the command. Missing or substituted vaults reject.
Registration pins the verified canonical vault address in the root's existing
allocation. Ordinary backing checks compare against that immutable address;
they do not repeat a PDA search. Token balances are still read fresh on every call.
Registration and wrap/unwrap already carry this vault and reuse their validated
observation. Accounting-only calls need no vault. The extra account adds a read
dependency on custody but no write permission or storage allocation.

`LedgerAccounts.root` accepts read-only access. For transfers, use
`Reader::transfer_writable_accounts` to determine exactly which Ledger records
need write access from a consistent snapshot. Same-polarity paths stop below
their lowest common ancestor, so that ancestor, the app group and token root
can remain read-only when unchanged. Opposite-polarity postings change both
gross columns through the token root. Changed absent endpoints require write
access for allocation. Zero-amount and self-transfers allocate no storage and
need no writable Ledger records, including when endpoints are absent. All
required checks still run, and zero-amount posting events are preserved. The
transaction fee payer remains writable. The helper reuses the core posting walk; it does not
grant authority or replace runtime validation. See [client planning](../../docs/READS.md#transfer-write-planning)
and executable minimal-permission examples in `tests/runtime/writable_accounts.rs`.

Tree mutations need write access to the parent whose child count changes,
including the token root when modifying its direct children. With generated
`LedgerAccounts` metas, explicitly mark that root writable for such operations
and for mint/burn transfers. Matching no-op mutations require no record writes.
Ledger creation requires a writable global Root. Later operations require writes
only to the Ledger records they change. Zero-amount custody operations avoid
endpoint allocation and allow read-only Ledger records, while retaining
their token/System calls, settlement checks and fixed account declarations.
Existing clients that supply extra writable accounts still work, but retain
those unnecessary transaction locks.

Clients can deserialize public `Record` data after its eight-byte `CVLEDG01`
header. Parent is at byte offset 40 for RPC filtering; filter registered records
when listing registered children. `LedgerAdded`, leaf/group creation and removal,
`Credit` and `Debit` report the original structural and posting semantics through
Anchor logs. They replace the draft `AccountChanged` snapshots. Only consume
events from successful transactions; see [event fields and compatibility](../../docs/EVENTS.md).

## Native SOL

`NATIVE_SOL` is the zero public key, also the System Program address. It cannot
be a token mint. It is SOL's logical ledger identity, regardless of the account
already occupying that address. SOL's root storage is `["ledger", zero, NATIVE_SOL]`,
with one protected Source and a System-owned, empty-data vault at
`["vault", root_storage]`. Only Ledger
can sign for that vault PDA. The root reports `TokenKind::Native`.

`add_native_sol` initializes name `SOL`, symbol `SOL` and decimals `9` and funds the vault to the
runtime's rent-exempt minimum for a zero-data account. An already-funded vault
is accepted. Any excess funding is surplus custody and creates no claim.
Root/Source storage funding and vault rent are separate from customer balances.
Matching repeated registration succeeds without changing state, funding storage
or emitting creation events.

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

Available backing is `vault.lamports - Rent::minimum_balance(0)`. Every mutation
checks that amount against all outstanding SOL claims. Withdrawals preserve the rent
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
| `TransferHook` | Supported, including a disabled hook; active hooks run during token custody transfers |
| `PermissionedBurn` | Supported; minting and burning remain token-program operations outside Ledger |
| Token-account `ImmutableOwner`, `TransferHookAccount` | Supported, including Token-2022 associated accounts and hook-enabled custody records |
| All other mint or token-account extensions | Rejected with `UnsupportedToken`; malformed data also rejects |

The rejected set includes transfer fees, permanent delegates,
confidential transfers, nontransferable/pausable tokens, default account states,
CPI guards and memo requirements. This version has no settlement mechanism for
those configurations. Fee extensions are rejected even when their
current fee is zero. Unknown extension data cannot
silently become supported. Registration also requires valid issuer name and
symbol metadata, which is stored with mint decimals. `LedgerView` reads that
snapshot without querying the issuer again; see [read interfaces](../../docs/READS.md).

For an active hook, append its program, canonical extra-account-metadata record
and declared records to the remaining accounts of `wrap`/`unwrap`. Include their
required write/signing permissions in the outer transaction, including when
calling through an application. The adapter uses the SPL resolver to pass only
the hook's declared records to Token-2022. Its ordinary transfer path is retained
when no hook is active. Hook configuration is read from the mint on every custody
call, so an issuer's update can change the required records or reject withdrawals.
Hook programs incur their own compute and account costs. Zero-amount custody
calls still execute an active hook. Internal Ledger transfers never call the
token program and therefore do not execute token hooks.

Token-2022 strips write access and signer privileges from the hook's four base
records (source, mint, destination and authority). Hook errors, failed exact
settlement and failed Ledger commits roll back token balances, hook state and
Ledger changes together. The full-backing guard still runs before mutation;
direct token transfers can restore missing backing without creating Ledger claims.

Permissioned burns require the configured burn authority as well as the token
owner or delegate under Token-2022's rules. The burn authority alone cannot burn
another holder's tokens. An application can withdraw tokens it controls and burn
them in the same transaction; a failed burn also rolls back the withdrawal.
Ledger neither burns tokens nor grants burn authority. Custody withdrawals first
retire the corresponding Source credit and debit balance under the existing rules.

Instruction data, account order, PDA seeds and Ledger record layouts are unchanged.
The Rust `RegisterToken.vault` field is now `UncheckedAccount`; the adapter
explicitly initializes and authenticates its token mint and authority using the
current Token-2022 decoder. This replaces Anchor's older allocation helper, which
cannot decode PermissionedBurn. Generated client account metas are unchanged.

Both token programs retain their own mint/freeze authority behavior. Runtime
settlement failures roll back token movement, accounting and new allocations.
Settlement instruction arguments and account order are unchanged; clients
select the appropriate token program. Registration and creation-event clients
must use the metadata interfaces above.

References: [Anchor token interface](https://www.anchor-lang.com/docs/tokens/basics/transfer-tokens)
and [Token-2022 extensions](https://solana.com/docs/tokens/extensions), including
[transfer hooks](https://solana.com/docs/tokens/extensions/transfer-hook) and
[permissioned burns](https://solana.com/docs/tokens/extensions/permissioned-burn).

## Supported scope and release status

- Native SOL, classic SPL Token and the Token-2022 configurations above are supported.
- Cross-custodian external-token transfers, off-chain intents, delegated app
  authority, upgrades/migration policy and rent reclamation are not implemented.
- Group cancellation preserves the original accounting walk. Unchanged roots
  and ancestors can be supplied read-only; clients select the write set before
  signing. Shared writable payers, wallets and other application state can still
  prevent parallel execution.
- No resource-based depth cap is imposed. Depth remains a checked `u8`, with
  root depth 2. Applications must budget their complete transactions, including
  other instructions and CPI calls. See [execution measurements](../../docs/EXECUTION_LIMITS.md)
  for compute, signed packet sizes, rent and client construction guidance.
- Group names require 1–64 UTF-8 bytes; registered leaf labels allow 0–64 bytes.
  An empty leaf label does not affect registration, identity or permissions.
  External mint decimals come from the mint.
  Internal units are raw integers; presentation precision is application policy.
- No deployed service, production program identity or upgrade authority has been
  selected. The checked-in ID and test-consumer identities are simulation-only.
- Tests establish the exercised behavior; they do not establish full spec parity
  or constitute a security audit. Do not treat this as a mainnet release merely
  because it builds.

Run `bash scripts/check.sh` with the pinned Rust and Agave tools on PATH. It
builds actual sBPF before running runtime tests; there is no native substitute.
