# Reusable Ledger core

- Follow original LedgerLib at cavalre-contracts commit
  `34d159ff4e88fdfdee16738d9a1228f0bf407212` and the root AGENTS.md.
- Keep this crate `no_std` and platform-independent. No Anchor, Solana public
  keys, PDAs, token SDKs, runtime storage or downstream application policy.
- Preserve recognizable LedgerLib functions and accounting behavior; the host
  supplies identity derivation and authenticated storage through interfaces.
- Keep pure accounting in this crate. Do not restore the old controller model
  or create a separate kernel without a concrete need.
