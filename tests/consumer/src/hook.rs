//! Test-only token hook and application mint/burn driver. Never deploy.
use anchor_lang::solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    instruction::Instruction,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
};
use spl_token_2022_interface::{
    extension::{transfer_hook::TransferHookAccount, BaseStateWithExtensions, StateWithExtensions},
    state::Account,
};
use spl_transfer_hook_interface::instruction::{ExecuteInstruction, TransferHookInstruction};

pub fn process(id: &Pubkey, a: &[AccountInfo], data: &[u8]) -> Option<ProgramResult> {
    if let Ok(TransferHookInstruction::Execute { amount }) = TransferHookInstruction::unpack(data) {
        return Some(execute(id, a, data, amount));
    }
    if let Some(bytes) = data.strip_prefix(b"token-mint") {
        return Some(token_action(id, a, bytes, false));
    }
    if let Some(bytes) = data.strip_prefix(b"token-burn") {
        return Some(token_action(id, a, bytes, true));
    }
    None
}
fn execute(id: &Pubkey, a: &[AccountInfo], data: &[u8], amount: u64) -> ProgramResult {
    let [from, mint, to, authority, metadata, state, ledger] = a else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if metadata.owner != id
        || state.owner != id
        || *metadata.key
            != spl_transfer_hook_interface::get_extra_account_metas_address(mint.key, id)
        || *state.key != Pubkey::find_program_address(&[b"hook-state", mint.key.as_ref()], id).0
        || *ledger.key != cavalre_ledger_solana::ID
    {
        return Err(ProgramError::InvalidAccountData);
    }
    spl_tlv_account_resolution::state::ExtraAccountMetaList::check_account_infos::<
        ExecuteInstruction,
    >(a, data, id, &metadata.try_borrow_data()?)?;
    // The token program must strip source/destination write rights and spending
    // authority signatures before calling an untrusted hook.
    if [from, mint, to, authority]
        .iter()
        .any(|v| v.is_writable || v.is_signer)
    {
        return Err(ProgramError::InvalidArgument);
    }
    for token in [from, to] {
        if token.owner != &spl_token_2022_interface::ID {
            return Err(ProgramError::IncorrectProgramId);
        }
        let data = token.try_borrow_data()?;
        let s = StateWithExtensions::<Account>::unpack(&data)?;
        if s.base.mint != *mint.key
            || !bool::from(s.get_extension::<TransferHookAccount>()?.transferring)
        {
            return Err(ProgramError::InvalidArgument);
        }
    }
    let mode = {
        let mut bytes = state.try_borrow_mut_data()?;
        if bytes.len() != 17 {
            return Err(ProgramError::InvalidAccountData);
        }
        let count = u64::from_le_bytes(bytes[1..9].try_into().unwrap()) + 1;
        bytes[1..9].copy_from_slice(&count.to_le_bytes());
        bytes[9..17].copy_from_slice(&amount.to_le_bytes());
        bytes[0]
    };
    match mode {
        0 => Ok(()),
        1 => Err(ProgramError::Custom(7101)),
        2 => invoke(
            &Instruction {
                program_id: *ledger.key,
                accounts: vec![],
                data: vec![],
            },
            std::slice::from_ref(ledger),
        ),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}
fn token_action(id: &Pubkey, a: &[AccountInfo], bytes: &[u8], burn: bool) -> ProgramResult {
    let [user, program, mint, wallet, authority] = a else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let (key, bump) = Pubkey::find_program_address(&[b"app", user.key.as_ref()], id);
    if !user.is_signer || *program.key != spl_token_2022_interface::ID || *authority.key != key {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let amount = u64::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| ProgramError::InvalidInstructionData)?,
    );
    let ix = if burn {
        spl_token_2022_interface::extension::permissioned_burn::instruction::burn_checked(
            program.key,
            wallet.key,
            mint.key,
            authority.key,
            authority.key,
            &[],
            amount,
            6,
        )?
    } else {
        spl_token_2022_interface::instruction::mint_to_checked(
            program.key,
            mint.key,
            wallet.key,
            authority.key,
            &[],
            amount,
            6,
        )?
    };
    invoke_signed(&ix, a, &[&[b"app", user.key.as_ref(), &[bump]]])
}
