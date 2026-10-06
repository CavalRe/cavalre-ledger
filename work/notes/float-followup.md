# Deferred: faithful Solana Float representation

Decision recorded: 2026-10-06.

Finish the Ledger design first. This note records a later Float change; it does
not authorize starting that implementation while Ledger remains the focus.

## Agreed direction

- Define Solana Float as a wrapper around `ethnum::I256`, stored in 32 bytes.
- Preserve the Solidity Float packing: a signed 72-bit mantissa in the low bits
  and the exponent in the remaining high bits.
- Preserve normalization, precision, rounding and arithmetic behavior. Any
  deliberate platform difference must be discussed explicitly.
- Consider optimizations only when they preserve the agreed semantics and have
  measured benefits.

## Current implementation

`CavalRe/cavalre-solana`, `crates/float/src/lib.rs`, currently stores an `i128`
mantissa followed by an `i32` exponent as 20 little-endian bytes. It already uses
`ethnum::I256` for intermediate calculations. This representation saves 12 bytes
per value but changes the encoding and constrains the exponent to `i32`.

The deferred change is to the Float representation and its semantic fidelity;
Ledger debits and credits were subsequently agreed as `u128`; Float work does
not change that decision.

Reference: `CavalRe/cavalre-contracts`, `math/FloatLib.sol`.
