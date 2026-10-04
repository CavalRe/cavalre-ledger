//! Read helpers corresponding to LedgerView.sol. Public record data can be read
//! directly by RPC clients; on-program readers validate ownership and identity.
use crate::ledger::{decode, Record};
use anchor_lang::prelude::*;

pub fn account(info: &AccountInfo) -> Result<Record> {
    decode(info)
}
pub fn debit_balance_of(info: &AccountInfo) -> Result<u128> {
    Ok(decode(info)?.debit)
}
pub fn credit_balance_of(info: &AccountInfo) -> Result<u128> {
    Ok(decode(info)?.credit)
}
