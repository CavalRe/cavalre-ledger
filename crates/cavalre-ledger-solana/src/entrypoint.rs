//! Cheap account parsing for existing-account transfers; Anchor handles allocation
//! and token settlement. Both paths use the same packed layout and posting rules.
use anchor_lang::Discriminator;
use core::mem::MaybeUninit;
use pinocchio::{account_info::AccountInfo, entrypoint::deserialize, MAX_TX_ACCOUNTS};
pinocchio::default_allocator!();
pinocchio::default_panic_handler!();
/// # Safety
/// The Solana loader supplies its validated, aligned instruction input buffer.
#[no_mangle]
pub unsafe extern "C" fn entrypoint(input: *mut u8) -> u64 {
    let mut accounts = [MaybeUninit::<AccountInfo>::uninit(); MAX_TX_ACCOUNTS];
    // The loader validates account counts and duplicate indices. Pinocchio
    // initializes exactly `count` entries, including aliases, without a heap.
    let (program, count, data) = deserialize(input, &mut accounts);
    if data.starts_with(crate::ledger_transfer::TRANSFER) {
        if !pinocchio::pubkey::pubkey_eq(program, crate::ID.as_array()) {
            return pinocchio::program_error::ProgramError::IncorrectProgramId.into();
        }
        let accounts = core::slice::from_raw_parts(accounts.as_ptr().cast(), count);
        return match crate::ledger_transfer::process(accounts, data) {
            Ok(()) => 0,
            Err(error) => error.into(),
        };
    }
    if data.starts_with(crate::instruction::CreateIdempotent::DISCRIMINATOR) {
        if !pinocchio::pubkey::pubkey_eq(program, crate::ID.as_array()) {
            return pinocchio::program_error::ProgramError::IncorrectProgramId.into();
        }
        let accounts = core::slice::from_raw_parts(accounts.as_ptr().cast(), count);
        match crate::ledger_transfer::existing_recipient(accounts, data) {
            Ok(true) => return 0,
            Ok(false) => (),
            Err(error) => return error.into(),
        }
    }
    let (program, accounts, data) = anchor_lang::solana_program::entrypoint::deserialize(input);
    match crate::entry(program, &accounts, data) {
        Ok(()) => 0,
        Err(error) => error.into(),
    }
}
