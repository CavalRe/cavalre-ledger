//! Test-only controller and transfer-hook consumer. Never deploy this fixture.
use anchor_lang::solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
};
use spl_token_2022_interface::{
    extension::{transfer_hook::TransferHookAccount, BaseStateWithExtensions, StateWithExtensions},
    state::Account,
};
use spl_transfer_hook_interface::instruction::TransferHookInstruction;

#[cfg(not(feature = "no-entrypoint"))]
anchor_lang::solana_program::entrypoint!(process);

pub fn process(id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    if data.first() == Some(&0) {
        let [user, program, remaining @ ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        if !user.is_signer || *program.key != cavalre_ledgers_solana::ID {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let (controller, bump) =
            Pubkey::find_program_address(&[b"controller", user.key.as_ref()], id);
        let instruction = Instruction {
            program_id: *program.key,
            data: data[1..].to_vec(),
            accounts: remaining
                .iter()
                .map(|a| AccountMeta {
                    pubkey: *a.key,
                    is_writable: a.is_writable,
                    is_signer: a.is_signer || *a.key == controller,
                })
                .collect(),
        };
        return invoke_signed(
            &instruction,
            remaining,
            &[&[b"controller", user.key.as_ref(), &[bump]]],
        );
    }
    let TransferHookInstruction::Execute { amount } = TransferHookInstruction::unpack(data)? else {
        return Err(ProgramError::InvalidInstructionData);
    };
    let [source, _, destination, _, _, counter] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    for token in [source, destination] {
        if *token.owner != spl_token_2022_interface::ID {
            return Err(ProgramError::InvalidAccountOwner);
        }
        let data = token.try_borrow_data()?;
        let state = StateWithExtensions::<Account>::unpack(&data)?;
        if !bool::from(state.get_extension::<TransferHookAccount>()?.transferring) {
            return Err(ProgramError::InvalidAccountData);
        }
    }
    if counter.owner != id || counter.data_len() != 8 {
        return Err(ProgramError::InvalidAccountData);
    }
    // A deterministic rejection verifies that both token and Ledger writes roll back.
    if amount == 13 {
        return Err(ProgramError::Custom(13));
    }
    let mut data = counter.try_borrow_mut_data()?;
    let value = u64::from_le_bytes(data[..].try_into().unwrap());
    data.copy_from_slice(
        &value
            .checked_add(1)
            .ok_or(ProgramError::ArithmeticOverflow)?
            .to_le_bytes(),
    );
    Ok(())
}
