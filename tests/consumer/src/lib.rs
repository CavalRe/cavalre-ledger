//! Test-only application CPI fixture. Never deploy this program.
use anchor_lang::solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
};
#[cfg(not(feature = "no-entrypoint"))]
anchor_lang::solana_program::entrypoint!(process);
pub fn process(id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let [user, program, rest @ ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !user.is_signer || *program.key != cavalre_ledger_solana::ID {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (authority, bump) = Pubkey::find_program_address(&[b"app", user.key.as_ref()], id);
    let instruction = Instruction {
        program_id: *program.key,
        data: data.to_vec(),
        accounts: rest
            .iter()
            .map(|a| AccountMeta {
                pubkey: *a.key,
                is_writable: a.is_writable,
                is_signer: a.is_signer || *a.key == authority,
            })
            .collect(),
    };
    invoke_signed(&instruction, rest, &[&[b"app", user.key.as_ref(), &[bump]]])
}
