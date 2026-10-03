# Ledgers on EVM

`contracts/Ledgers.sol` ports the portable service's namespaces, controller
permissions, account lifecycle, gross double-entry accounting and custody rules
to Solidity. `LedgerVault.sol` holds ERC20 collateral separately for each
namespace/token pair. Amounts use exact token base units, bounded by `uint128`
to match the portable core. Accounts are at most 16 edges below their root.

This implementation is a directly deployed, non-upgradeable service with its own
storage. It has no global owner, Dispatcher dependency or token wrapper surface.
It has not been deployed or audited. Integrating it into `cavalre-contracts`,
including any Dispatcher adapter and state migration, is separate work.

## Identity and lifecycle

Any caller can create a namespace. Its identity derives from the creator and a
caller-selected `bytes32` relative key. The creator may register journal roots
and ERC20 asset roots. A namespace has at most one Asset root for a given token.
Journal roots derive from namespace and relative key; account identities derive
from root, absolute parent, and relative key. Public identity helpers expose each
derivation. All identities are accounting keys, not EVM account addresses.

A controller is an EOA or smart contract address. Root, parent, kind and controller
are immutable for an account's lifetime. There are no controller setters, broad
operator approvals or inherited spending permissions. Controllers can close
only their own empty, childless accounts. Both gross sides must be zero, even if
the net balance is zero. Roots and namespaces have no closure operation.

Creation/recreation increments a retained account incarnation. Closing an account
does not delete its incarnation counter. Pending consent for an old account or
parent cannot authorize operations on its replacement.

## Operations and authority

| Operation | Required controller(s) |
| --- | --- |
| `createNamespace(relative)` | Caller becomes namespace controller |
| `createJournal(namespace, relative)` | Namespace controller calls |
| `registerAsset(namespace, token)` | Namespace controller calls |
| `createNode(root, parent, relative, controller, kind)` | Parent controller and new account controller; namespace controller substitutes for parent at root |
| `openPosition(root)` | Caller creates its own direct Asset debit leaf without namespace consent |
| `postJournal(from, to, amount)` | Both endpoint controllers |
| `transferJournal(from, to, amount)` | Source controller calls; both endpoints must be direct debit leaves |
| `transferClaims(from, to, amount)` | Source controller calls; direct or nested Asset debit leaves |
| `deposit(position, amount)` / `withdraw(position, amount)` | Position controller calls; direct Asset debit leaves only |
| `closeNode(account)` | Account controller calls |

Only leaves are posting endpoints. Asset roots cannot create credit leaves or
accept journal postings. Journals have no native redemption route. Zero amounts
still require valid accounts and authorization. Public-style transfers check
funds even for self-transfers; an authorized journal self-posting is a no-op.

Namespace and parent controllers can organize accounts but cannot spend balances
controlled by descendants. A contract controller acts by calling Ledgers from
its own address, subject to its own policy. No application allowlist is needed.
Incoming source-authorized transfers require no recipient consent; applications
must not interpret an unsolicited receipt as an authorized application action.

## Joint consent on EVM

An ordinary EVM call authenticates one immediate caller. For operations needing
two distinct controllers, one can preapprove the exact operation and the other
can execute it. Both can also preapprove and let a relayer execute it.

1. Obtain `creationOperation(...)` or `journalOperation(...)` from the service.
2. Each controller who will not be the executing caller invokes
   `approveOperation(operation, deadline)` from its own address.
3. Submit the matching `createNode(...)` or `postJournal(...)` call before expiry.

The operation hash binds the chain ID, service address, operation selector, all
action parameters and relevant account incarnations. A controller approves one
execution of those exact parameters; approving the same hash again replaces its
deadline and does not accumulate executions. A deadline of zero revokes approval;
otherwise it is a Unix timestamp, valid through that timestamp inclusively.

Successful execution consumes all participating approvals, including any stored
approval belonging to the executing caller. A failed operation rolls approval
consumption back with its balance and lifecycle changes. There is no signed
offchain permit or general relayed transfer API in this version; EVM transaction
nonces protect direct caller authorization. Anyone may execute an operation once
all necessary controllers have approved its exact hash. Use a deadline appropriate
to the intended action; consent does not bind the account's intermediate balance.

For example, Alice and a group's controller can create Alice's leaf as follows:

```solidity
// Alice obtains this digest, then submits approveOperation from her own address.
bytes32 operation = ledgers.creationOperation(
    root, group, relative, alice, Ledgers.Kind.DebitLeaf
);
// Alice submits: ledgers.approveOperation(operation, deadline);
// The group controller then submits:
bytes32 leaf = ledgers.createNode(root, group, relative, alice, Ledgers.Kind.DebitLeaf);
```

The group controller cannot subsequently spend or close `leaf`. General journal
postings involving it require Alice's consent; claim transfers out require a call
from Alice. A smart-account controller can compose multiple calls atomically.

## Accounting and custody

Accounts hold one authoritative packed pair of gross debit/credit balances.
Roots hold one gross supply, from which both root sides are derived. Parent
counts are maintained on nested creation and closure. Posting paths are loaded
from immutable service storage; callers never supply authority records or balance
snapshots. Same-side shared ancestors cancel before arithmetic. Group polarity
does not change the balance side propagated from an endpoint.

Registering an ERC20 deploys a dedicated vault with immutable token and Ledgers
addresses. The vault has no administrative withdrawal, arbitrary call, allowance
or sweep operation. Direct token donations increase collateral without claims.
The namespace controller cannot access another account's claims or its vault.

For deposits, the controller approves the **Ledgers service**, opens a direct
position with `openPosition`, and calls `deposit`. The service transfers from
that same controller into the root's vault and checks both the exact wallet
debit and exact vault receipt before issuing claims. Withdrawals require the
vault to cover **all** claims, debit the caller's claims, send to that controller,
and check both exact native deltas. Any failure rolls back all changes. All
mutating service entrypoints reject reentrancy during custody settlement.

Nested custody is funded through a direct position followed by `transferClaims`.
For redemption, move the claim back to a direct position first. A source controller
must authorize every transfer. Internal claim transfers never move ERC20 tokens
or invoke token hooks, and leave the vault and aggregate claim supply unchanged.

Only ERC20 custody is implemented; use wrapped native tokens for native currency.
Standard boolean-returning and no-return ERC20 transfers are accepted when their
observed deltas are exact. Fee transfers, no-op transfers and settlement-time
balance changes fail the delta checks. EVM tokens do not expose Solana's uniform
extension policy: registration cannot guarantee future token behavior. Rebasing,
seizable, upgradeable, freezing or dishonest-balance tokens retain their own
issuer/implementation risks. A collateral shortfall blocks withdrawals until
backing is restored; this service has no collateral recovery or loss allocation
mechanism. Applications must select appropriate underlying tokens.

## Verification

`bash scripts/check-solidity.sh` checks formatting, production bytecode size and
the Foundry suite. The suite reads the unchanged 162-action hierarchy and
138-action custody fixtures, compares success/revert outcomes and all resulting
balances, and checks independent group sums, collateral and token conservation.
Additional tests exercise controller separation, expiring/revoked consent,
chain/service/parameter binding, recreation replay protection, smart-contract
controllers, maximum depth and amount bounds, and adversarial token rollback.

Run `bash scripts/check.sh` independently for the Rust and actual sBPF suites.
Neither gate deploys a program or migrates the existing Solidity Ledger.
