# Portable Ledger core

`crates/ledger-core` (`cavalre-ledgers-core`) is the shared Ledger service layer.
It is `no_std` with `alloc` and depends only on `cavalre-ledgers-kernel`. The
standalone Solana service in `crates/ledger-solana` now uses it for namespace and
account lifecycle, controller-authorized postings, and native custody accounting.
It can also be used by a Commonware state machine without Anchor or Solana.

The core is not a second ledger or a mirror of principal balances. `Root` and
`Account` are views of the host's authoritative records. The existing arithmetic
kernel still computes the checked double-entry changes. Root debit and credit
are derived from one gross supply; groups preserve both current gross sides.

## Responsibilities

| Shared core | Host adapter |
| --- | --- |
| Namespace and structural controller permissions | Authenticate signatures and application capabilities |
| Immutable account topology and bounded complete paths | Derive canonical identities; authenticate record ownership, versions and types |
| Endpoint spending permissions and root-type restrictions | Load current state from the transaction's authenticated storage view |
| Empty, childless closure and parent child-count changes | Enforce unused identities on creation; allocate/delete records and refund the original payer |
| Checked posting plans and cancellation at shared ancestors | Check write access and native amount bounds; commit each changed record once |
| Custody supply changes, solvency before withdrawal, exact deltas | Authenticate native asset, vault and wallet; enforce token policy; perform and reload transfers |
| Deterministic `u128` accounting | Atomically commit or roll back all storage and external effects |

The Solana adapter retains its `u64` native-token limits, account layouts, PDA
identities, instruction encoding, event schema, Token-2022 support policy and
original rent recipients. This extraction changes program bytecode, so it
requires a fresh build and review before any deployment. It does not deploy or
migrate accounts. The older bundled prototype remains in `CavalRe/cavalre-solana`.

## Authority and operations

`Authorization::from_verified_signers` accepts identities **already verified by
the host for this invocation**. It does not verify a signature or elevate an
identity supplied by a client. On Solana these identities come from Anchor
`Signer` accounts, including runtime-authenticated CPI PDA signers. A dexchain
adapter must derive them from verified transaction signatures and explicitly
authorized application execution. A controller ID in transaction data is not
proof of control.

- Any authenticated creator can initialize its namespace and register roots.
- `create_node` requires the new controller and the parent's structural
  controller. At a root, the namespace creator has structural authority.
- `open_position` lets a holder open its own direct Asset position without
  namespace consent.
- `PostJournal` requires both endpoint controllers. Journal debit and credit
  leaves may be nested; a self-posting is an authorized no-op.
- `TransferJournal` requires the source controller and direct debit leaves.
- `TransferClaims` requires the source controller and Asset debit leaves,
  including nested claims. Transfers check funds even when sending to self.
- `close_node` requires the account controller, zero gross balances and no
  children, and plans the parent count decrement. Root closure is not exposed.

The namespace creator and a group's controller cannot spend descendant claims.
Groups cannot be posting endpoints. Asset roots cannot create credit leaves or
accept journal issuance. Journals have no native redemption route. Source-only
transfers allow unsolicited incoming claims; applications must not treat a
receipt as authorized staking or liquidity provision.

## Planning and atomicity

`plan_posting` receives the exact unique union of both complete ancestry paths,
excluding the root. The endpoint indexes select authenticated records, not
client-provided parentage. It validates root membership, account shape, depth,
cycles, missing/duplicate/extraneous records and the operation's controllers.
Each path has at most 16 edges; at most 32 non-root records are accepted.
Zero amounts still undergo validation and authorization.

A successful `Posting` contains each actual balance change once, with before
and after values, plus endpoint/custody identities for reporting. Same-side
shared ancestors cancel before arithmetic and need no write. Failed planning
has no effects. `Creation` and closure similarly return parent count changes
without mutating state.

The host must apply a plan within the same atomic transaction and state snapshot
that produced it. All records read to validate topology and authority must stay
stable through commit, using serial execution, locking or read-set validation.
Check every write privilege and representation bound before writing; do not
apply a partially valid plan. Plans are not signed authorizations, persistent
commands or replayable transactions. Transaction signatures, nonces, replay
protection and authenticated state commitments remain host responsibilities.

For custody, `prepare_deposit` and `prepare_withdrawal` return a `CustodyPlan`.
The input balances must be fresh observations of authenticated native custody
accounts, with vault and wallet distinct and the same asset and amount units.
The host enforces supported token behavior and transfer authority.

On deposit, transfer first, reload both native accounts, call
`verify_settlement`, then mint exactly the observed claim amount. On withdrawal,
preflight checks that the vault covers **all** outstanding claims; debit claims,
transfer, reload, and verify both exact deltas. Any transfer, verification or
commit failure must roll back the entire operation. A host without transactional
external transfers cannot safely implement this adapter contract by writing
claims and calling an asynchronous bridge. Donations remain surplus collateral
and never issue claims.

## SR and dexchain integration

The core has no SR, rewards, share-token, Multiswap, consensus or storage-engine
dependency, and no reward hooks. A consuming economic module authorizes its
actions and explicitly settles dependent entitlements before principal changes.
Every route into and out of its controlled accounts must respect that policy;
generic Ledger transfers alone are not an SR integration.

The Commonware adapter can map its authenticated records to these views and
apply successful plans through its existing atomic state transition. That
adapter, SR integration and weighted validator admission remain separate work.
No Commonware consensus changes are part of this extraction.

## Verification

Run `cargo test -p cavalre-ledgers-core --locked` for core tests. They replay all
162 hierarchy and 138 custody actions from the pinned Solidity fixtures, compare
every resulting balance and rejection, and independently check group aggregates,
claim supply, collateral and native-token conservation. Additional cases cover
authority, malformed paths, root separation, lifecycle, maximum depth, exact
settlement and arithmetic bounds.

`bash scripts/check.sh` also builds the actual Solana programs and runs the
Solana runtime suite, including native token behavior, CPI controller
authority, atomic rollback and maximum-depth transactions. The fixture inputs
and economic arithmetic kernel are unchanged.
