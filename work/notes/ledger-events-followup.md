# Deferred: reduce Ledger event costs

Decision recorded: 2026-10-06.

Keep all current Ledger events for now, including the per-posting `Credit` and
`Debit` events. Removing them is an option to revisit, not an agreed change.

## Potential savings

At main commit `5d6d947`, the existing-recipient classic-token sibling transfer
measures **1,934 CU**, including two 104-byte posting events. Under the pinned
runtime schedule, their logging syscalls cost **608 CU** in total, about 31% of
the transfer, before payload preparation.

Subtracting those syscall charges gives **1,326 CU**. This is arithmetic, not a
measured no-events benchmark. Removing payload preparation and any compiler
effects must be measured in a fresh build. Deeper transfers emit more posting
events and could save more.

The comparison with p-token prompted this discussion: its published 105-CU
`TransferChecked` sample excludes program logging and does not include structured
transfer events.

## Tradeoff and follow-up

- Ledger balances, authorization and backing checks do not depend on event logs.
  Removing logs must preserve all accounting and validation behavior.
- Posting events provide the affected account, amount and resulting gross
  balance at each step of the ancestor walk. Without them, reconstructing the
  journal requires a Ledger-aware indexer that tracks state and applies the
  accounting rules to successful executions, including relevant inner calls.
- Standard RPC token-balance metadata does not describe Ledger's custom debit
  and credit records. Account reads alone provide current state, not the full
  historical journal. Existing logs can also be truncated; they are not a
  guaranteed archival journal.
- Before revisiting removal, identify event consumers, define how history will
  be reconstructed, and measure the same sibling and ancestor-walk fixtures
  with and without posting logs. Applications can choose higher-level events
  independently of Ledger's per-posting logs.

References: [current event contract](../../docs/EVENTS.md),
[current cost profile](../../docs/TRANSFER_COSTS.md),
[p-token proposal](https://github.com/solana-foundation/solana-improvement-documents/blob/main/proposals/0266-efficient-token-program.md).
