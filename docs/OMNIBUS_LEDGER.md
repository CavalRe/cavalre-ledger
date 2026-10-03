# Ledger: independent omnibus custody and accounting

`crates/cavalre-ledgers-solana` is the standalone Ledger service. It depends on the shared
`crates/cavalre-ledgers-core` portable service layer, its `crates/cavalre-ledgers-kernel` posting
kernel, and the native token interfaces. See [Ledger core](LEDGER_CORE.md) for
the shared rules and host adapter contract. It has no
SR, reward, share, Multiswap, or Launchpad dependency. The checked-in program
identity is for simulation; this change does not deploy it.

## Native tokens and internal claims

An omnibus vault holds a native asset for multiple internal accounts. Ledger
creates one vault per namespace and mint. The native token program owns the
vault data; the Ledger Asset PDA is its spending authority.

- **Wrap (`deposit`):** transfer tokens into the vault, verify exact receipt
  and sender debit, and create an internal custody claim atomically.
- **Unwrap (`withdraw`):** cancel a claim and return exactly that many native
  base units, with complete transaction rollback on failure.
- **Internal transfer:** move existing custody claims without changing vault
  tokens, native mint supply, or aggregate claim supply.

There is no externally transferable wrapper mint. A vault balance measures
collateral; an internal balance allocates a claim on that collateral. Native
token interfaces expose the vault, while Ledger accounts expose the internal
allocations. Those accounts remain publicly readable onchain.

`Asset.total_claims` represents root debit, root credit, and implicit Source
credit once. The native vault amount is read directly and never cached.
Donations create surplus collateral without claims. Before any withdrawal,
the vault must cover all outstanding claims.

## Identity and authority

Any signer can create its own namespace at `ledger / creator / id`. Its creator
registers assets and journals. These are local structural capabilities, with
no inherited right to spend another controller's accounts.

| Operation | Required authority |
| --- | --- |
| Create namespace | Creator and rent payer, currently the same signer |
| Register asset or journal | Namespace creator |
| Create group or leaf | Parent structural authority, new controller, rent payer |
| Deposit or withdraw direct custody | Holder |
| Transfer existing custody claims | Source holder/controller |
| Post a journal entry | Both endpoint controllers |
| Transfer between direct journal debit leaves | Source controller |
| Close an empty account | Its controller; node rent returns to its original payer |

A controller may be a wallet or an application PDA signing through CPI. Ledger
does not interpret the application's economic policy. Source-authorized
transfers permit incoming credits without recipient consent; applications must
not interpret an unsolicited custody receipt as an authorized staking action.
General journal postings require both endpoint controllers. An application's
settlement design must account for every instruction that can change its state.

Use the crate's `cpi` feature for Anchor callers, or its generated instruction
data and account metas for native callers. Controllers sign with their own
program's PDA seeds. There is no CavalRe application allowlist.

## Hierarchy and journals

The service supports Asset roots and Journal roots. Nodes have immutable root,
parent, relative identity, controller, and kind, plus current debit and credit
balances. There are no SR membership or reserved-account fields in this program.

Asset leaves are fully backed debit claims. Journals support both debit and
credit leaves, but have no native redemption route. Journal issuance cannot
create withdrawable custody claims.

Posting uses the existing checked accounting kernel. Callers supply exactly
the unique union of both complete ancestry paths. Shared same-side ancestors
cancel and remain read-only. Groups are not transfer endpoints. A node can
close only when both gross balances and its child count are zero.

To fund a nested custody leaf, compose `deposit` and `transfer_nested` in one
transaction. To withdraw, reverse that route. Each source controller must sign;
a later failure rolls back earlier token movements and hierarchy writes.

## Native token support policy

Legacy SPL Token, including wrapped SOL, and Token-2022 are supported. Claim
amounts always use exact native base units. Wrapped SOL remains wrapped on exit.

Supported Token-2022 mint extensions:

- Metadata and group pointers, token metadata, token groups and group membership.
- Transfer hooks whose update authority is disabled; the hook program may still
  have its own upgrade authority. Required accounts are resolved from the hook's
  standard extra-account metadata and supplied on deposit/withdrawal.
- Permissioned burn. The configured authority is an additional burn signer,
  not a replacement for the token holder. Ledger exposes no vault burn route.

Vaults have ImmutableOwner; hook mints also require TransferHookAccount. Wallet
account extensions are limited to those same two types. Transfer fees,
permanent delegates, confidential balances, mutable hook configurations and
all other extensions are rejected. The policy is checked at registration and
on each native custody movement. Existing mint freeze authority remains able
to block native transfers; a failed CPI rolls the transaction back.

Only deposit/withdrawal invokes a native transfer hook. Internal claim transfers
do not invoke it and do not transfer native reward entitlements between holders.

## Relationship to the existing prototypes

`CavalRe/cavalre-solana/programs/cavalre` retains the bundled Ledger/share/SR prototype and its existing
account layouts for regression coverage. The standalone service has a distinct
program identity and its own accounts. No account migration or reinterpretation
is performed. Both programs use the same pure posting kernel; the standalone
service additionally uses the portable Ledger service layer.

The current native SR and Multiswap programs are not yet consumers of this
service. SR integration must define vault registration, internal beneficiary
eligibility, reward accounting during custody, and wrap/unwrap settlement.
Collateral and its claims must not both count as eligible principal.

The Ledger kernel does not call SR. A token hook called during Ledger's native
transfer also cannot call back into Ledger through indirect reentry. Operations
that need both services must respect that boundary and compose settlement in
an appropriate consuming application/coordinator.

## Verification

`bash scripts/check.sh` builds the standalone program and a test-only external
consumer as actual sBPF artifacts. The `solana` suite reuses the custody and
hierarchy cases, including the 138-action custody and 162-action hierarchy
reference fixtures. Additional cases cover Token-2022 custody, extension
rejection, permissioned-burn authority, hook execution/rollback, independently
controlled namespaces, and two external applications controlling PDA claims.

The consumer fixture is under `tests/solana-consumer`; never deploy it. Tests
establish the covered behavior, not an audit, production deployment, or completed
SR/Multiswap integration. See [accounting](ACCOUNTING.md) and [the core contract](LEDGER_CORE.md)
for the shared posting rules and ancestry bounds.
