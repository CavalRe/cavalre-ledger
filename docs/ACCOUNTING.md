# Ledger accounting

Each account has current gross debit and credit balances, **D** and **C**.
A group's balances equal the sums of its immediate children's corresponding
sides. At a root, **D = C = gross supply**, represented by one authoritative
supply value. Gross balances are current allocations, not cumulative turnover.

A posting `from → to` means credit the source and debit the destination:

| Leaf polarity | Source effect | Destination effect |
| --- | --- | --- |
| Debit | Decrease D | Increase D |
| Credit | Increase C | Decrease C |

The endpoint's side propagates through every ancestor. A group's own polarity
changes its normal-balance presentation, not the side propagated through it.
Shared same-side ancestors cancel before arithmetic and need no write.
Opposite-side postings preserve both gross changes. Every changed record is
included once with checked before/after values.

The portable kernel uses checked `u128`, at most 16 edges per path, and rejects
malformed ancestry, cycles, inconsistent shared snapshots, overdrafts and
overflow. Internal self-postings are no-ops after validation; public-style
transfers also check the sender's funds before accepting a self-transfer.

`cavalre-accounting` supplies arithmetic only. `cavalre-ledger-core` adds the
controller and root-type rules. Complete paths are loaded from authenticated
host state. Groups are not spending endpoints; namespace and parent controllers
cannot spend another controller's leaves. Journal postings require both endpoint
controllers, while permitted debit transfers require source consent.

Asset roots represent fully backed custody claims. Their implicit Source credit
changes only at the exact deposit/withdrawal boundary; their explicit leaves
are debit claims. Journal roots may contain credit leaves and issue internal
units through coauthorized postings, but have no native redemption route.
Solana preserves its narrower `u64` native-token bounds.

Nodes have immutable root, parent, kind and controller for their lifetime. A
node closes only when both gross balances and its child count are zero.
Parent counts and record creation/deletion change atomically. On Solana,
storage rent returns to the original creation payer.

The [core adapter contract](LEDGER_CORE.md) defines authenticated state,
signatures, atomic commit and custody observations. The [Solana service
specification](OMNIBUS_LEDGER.md) defines account identity and supported native
token behavior. SR settlement is explicit application logic above Ledger.
