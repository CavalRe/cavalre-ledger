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
    if data == b"metadata" {
        use anchor_lang::AnchorSerialize;
        use cavalre_ledger_solana::ledger_view::Reader;
        let root = accounts
            .first()
            .ok_or(ProgramError::NotEnoughAccountKeys)?
            .key;
        let reader = Reader::from_account_infos(accounts)?;
        let symbol = reader
            .symbol(root)
            .map_err(|_| ProgramError::InvalidAccountData)?;
        let decimals = reader
            .decimals(root)
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
    if let Some(data) = data.strip_prefix(b"custody-helper") {
        return custody_helper(
            *program.key,
            rest,
            &[&[b"app", user.key.as_ref(), &[bump]]],
            data,
        );
    }
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

// Exercise the public typed helpers separately from the original raw forwarding
// path, so runtime regression cases continue to cover both calling interfaces.
fn custody_helper(
    program: Pubkey,
    accounts: &[AccountInfo],
    seeds: &[&[&[u8]]],
    data: &[u8],
) -> ProgramResult {
    use anchor_lang::{prelude::CpiContext, AnchorDeserialize, Discriminator};
    use cavalre_ledger_solana::{instruction, ledger_cpi as cpi};
    #[derive(AnchorDeserialize)]
    struct Args {
        parent: Pubkey,
        relative: Pubkey,
        amount: u64,
    }
    let args = Args::try_from_slice(data.get(8..).ok_or(ProgramError::InvalidInstructionData)?)
        .map_err(|_| ProgramError::InvalidInstructionData)?;
    let is_token = data.starts_with(instruction::Wrap::DISCRIMINATOR)
        || data.starts_with(instruction::Unwrap::DISCRIMINATOR);
    if is_token {
        let [payer, authority, funding_authority, root, mint, vault, wallet, token_program, system_program, remaining @ ..] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        let ctx = CpiContext::new_with_signer(
            program,
            cpi::accounts::MoveTokens {
                payer: payer.clone(),
                authority: authority.clone(),
                funding_authority: funding_authority.clone(),
                root: root.clone(),
                mint: mint.clone(),
                vault: vault.clone(),
                wallet: wallet.clone(),
                token_program: token_program.clone(),
                system_program: system_program.clone(),
            },
            seeds,
        )
        .with_remaining_accounts(remaining.to_vec());
        if data.starts_with(instruction::Wrap::DISCRIMINATOR) {
            cpi::wrap(ctx, args.parent, args.relative, args.amount)?;
        } else {
            cpi::unwrap(ctx, args.parent, args.relative, args.amount)?;
        }
    } else {
        let deposit = data.starts_with(instruction::WrapSol::DISCRIMINATOR);
        if !deposit && !data.starts_with(instruction::UnwrapSol::DISCRIMINATOR) {
            return Err(ProgramError::InvalidInstructionData);
        }
        let [payer, authority, funding_authority, root, vault, wallet, system_program, remaining @ ..] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        let ctx = CpiContext::new_with_signer(
            program,
            cpi::accounts::MoveSol {
                payer: payer.clone(),
                authority: authority.clone(),
                funding_authority: funding_authority.clone(),
                root: root.clone(),
                vault: vault.clone(),
                wallet: wallet.clone(),
                system_program: system_program.clone(),
            },
            seeds,
        )
        .with_remaining_accounts(remaining.to_vec());
        if deposit {
            cpi::wrap_sol(ctx, args.parent, args.relative, args.amount)?;
        } else {
            cpi::unwrap_sol(ctx, args.parent, args.relative, args.amount)?;
        }
    }
    Ok(())
}
