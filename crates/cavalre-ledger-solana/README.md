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
and logical/physical address helpers live in `ledger_lib.rs`; `ledger_view::Reader` queries a
validated account snapshot without invoking Ledger. Existing exports from
`ledger` remain available when mutations are enabled. See [read usage](../../docs/READS.md).

## Accounts and authority

The reusable core retains the original effective flags, custody ancestry,
registration, checked posting walk and shared-ancestor cancellation. The Solana
host authenticates signers and storage, commits changes and performs token calls.
Failures propagate to the runtime for atomic rollback.

`to_address(parent, relative)` returns `Keccak256(parent32 || relative32)`.
There is no program ID, prefix, bump or curve test in logical child identities.
`to_address_by_name(parent, name)` first hashes the exact UTF-8 name. `SOURCE`
is the precomputed full hash of `"Source"`. Relative identities retain their
permission role; a derived absolute address is a lookup key, not signing authority.

Each registered group has one physical container. Its leaves are mapping entries
inside that container. `account_storage_address(parent, relative)` returns the
physical group PDA; it is not the logical child helper. Leaf receipt needs no
recipient signature, registration, keypair or separate PDA. The transaction payer
funds required container growth and receives no authority over the balance.

External ledger roots use their mint as logical identity and
`root_storage_address(zero, mint)` as physical storage. Native SOL uses the
reserved `NATIVE_SOL` identity. Accounting-only roots retain the authority-scoped
`["ledger", authority, identifier]` identity; the signed authority controls
issuance and postings. External direct children use their relative signer; nested
children use that branch's custodian authority. Applications sign with PDAs via
CPI. Tree mutations require runtime signatures, not off-chain intents.

All applications share a token tree and its custody `["vault", root_storage]`.
Each external/native ledger has one protected Source credit leaf in its root
mapping. Other external accounts are debits. Accounting-only ledgers can contain
credit accounts and have no native redemption route.

Group labels require 1–64 UTF-8 bytes; explicit leaf labels allow 0–64 bytes.
Named helpers require 1–64 bytes, with no normalization. A name hash grants no
signing authority. Matching registration is idempotent. Group admission policy
remains fixed while registered; incompatible metadata or balances reject.
Removal requires zero gross balances and no registered children, and retains
allocated storage. Registration remains distinct from allocation.

## Global Root and child indexes

The fixed global `Root` is a debit group at depth 1. Its ordinary children are
ledger groups at depth 2; each has a Source. Posting stops at the selected ledger,
so global Root's balances stay zero. Root initializes on first successful ledger
creation, without granting its payer global authority.

Creation supplies writable global and ledger containers. Source and child indexes
are stored inside them; no Source PDA or append-slot PDA is supplied. Repeated
matching creation performs no writes or allocation. Ordinary posting needs no
global Root input.

Every group maintains the original insertion-ordered child vector and one-based
reverse indexes. Removal swaps the last child into the removed position and
updates its reverse index atomically. Supply that child's group container if it
is a group; mapped leaves already reside in the parent. Implicit leaves have no
registered index. Root lists absolute ledger identities; other groups list
relative identities. Discovery reads Root's vector, with no separate registry.

A group container is 528 base bytes plus 288 bytes per mapping capacity slot.
Binary search locates a logical key. Mutation decodes requested leaves only;
insertions and growth can move existing bytes. Group references point to separate
group containers with one authoritative balance store. Capacity is retained on
removal. Clients fetch parent containers for reads, not separate leaf/index PDAs.
See [READS.md](../../docs/READS.md) for layout, client helpers and absence semantics.

This replaces the previous draft layout and account lists. No existing deployment
is migrated. Logical cancellation is preserved, but siblings now share a physical
write lock. Containers are not paged, so physical size and insertion cost limit
large groups. These regressions and fresh measurements are documented in
[TRANSFER_COSTS.md](../../docs/TRANSFER_COSTS.md).

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

Supply endpoint parent containers, group endpoints, custody ancestors and posting
ancestors once each, excluding the fixed root. New groups need their physical
container address for allocation. Leaves and Source need no separate account meta.
The program verifies owners, physical identities and logical root membership. See executable
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
need write access from a consistent snapshot. Same-polarity paths preserve the
common ancestor balance, but any container holding a changed leaf must be writable.
An unchanged token root can remain readonly for transfers within an app group.
Opposite-polarity postings change both gross columns through the token root.
Changed absent endpoints require writable parent containers for map growth. Zero-amount and self-transfers allocate no storage and
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

Clients decode group headers with `ledger_lib` and mapped leaves with
`ledger_storage` or `record_at`. Enumerate the parent child vector for registered
children; RPC filters over group headers cannot enumerate mapped leaves. `LedgerAdded`, leaf/group creation and removal,
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

The token custody instruction data and fixed custody accounts retain their
meaning; mapped Ledger inputs follow the container rules above.
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
- Group cancellation preserves the original accounting walk. An ancestor
  container is readonly only when neither its header nor mapped leaves change.
  Clients select the physical write set before signing. Shared writable payers, wallets and other application state can still
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
