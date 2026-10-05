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
    if let Some(address) = data.strip_prefix(b"metadata") {
        use anchor_lang::AnchorSerialize;
        use cavalre_ledger_solana::ledger_view::Reader;
        let root = Pubkey::new_from_array(
            address
                .try_into()
                .map_err(|_| ProgramError::InvalidInstructionData)?,
        );
        let reader = Reader::from_account_infos(accounts)?;
        let symbol = reader
            .symbol(&root)
            .map_err(|_| ProgramError::InvalidAccountData)?;
        let decimals = reader
            .decimals(&root)
            .map_err(|_| ProgramError::InvalidAccountData)?;
        let mut bytes = Vec::new();
        (symbol, decimals).serialize(&mut bytes)?;
        anchor_lang::solana_program::program::set_return_data(&bytes);
        return Ok(());
    }
    let [user, program, rest @ ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !user.is_signer || *program.key != cavalre_ledger_solana::ID {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (authority, bump) = Pubkey::find_program_address(&[b"app", user.key.as_ref()], id);
    let custody = data.strip_prefix(b"custody-helper");
    let data = custody.unwrap_or(data);
    let mut instruction = Instruction {
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
    if custody.is_some() {
        cavalre_ledger_solana::ledger_cpi::set_custody_root_writable(&mut instruction)?;
    }
    invoke_signed(&instruction, rest, &[&[b"app", user.key.as_ref(), &[bump]]])
}
