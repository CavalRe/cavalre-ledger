//! Custody CPI helpers with amount-dependent root access.
//! Use the same Anchor context, accounts, remaining records and signer seeds as
//! the generated CPI functions. The outer instruction must supply every write
//! required by the inner call; these helpers cannot grant runtime privileges.
use anchor_lang::{
    prelude::*,
    solana_program::{instruction::Instruction, program::invoke_signed},
    InstructionData,
};

pub use crate::cpi::accounts;

/// Deposit classic SPL or Token-2022 assets, with a root write only if nonzero.
pub fn wrap<'info>(
    ctx: CpiContext<'_, '_, '_, 'info, accounts::MoveTokens<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    amount: u64,
) -> Result<()> {
    invoke(
        ctx,
        crate::instruction::Wrap {
            parent,
            relative,
            amount,
        }
        .data(),
        amount,
    )
}

/// Withdraw classic SPL or Token-2022 assets through the existing Ledger entry point.
pub fn unwrap<'info>(
    ctx: CpiContext<'_, '_, '_, 'info, accounts::MoveTokens<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    amount: u64,
) -> Result<()> {
    invoke(
        ctx,
        crate::instruction::Unwrap {
            parent,
            relative,
            amount,
        }
        .data(),
        amount,
    )
}

/// Deposit native SOL, with a root write only if nonzero.
pub fn wrap_sol<'info>(
    ctx: CpiContext<'_, '_, '_, 'info, accounts::MoveSol<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    amount: u64,
) -> Result<()> {
    invoke(
        ctx,
        crate::instruction::WrapSol {
            parent,
            relative,
            amount,
        }
        .data(),
        amount,
    )
}

/// Withdraw native SOL through the existing Ledger entry point.
pub fn unwrap_sol<'info>(
    ctx: CpiContext<'_, '_, '_, 'info, accounts::MoveSol<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    amount: u64,
) -> Result<()> {
    invoke(
        ctx,
        crate::instruction::UnwrapSol {
            parent,
            relative,
            amount,
        }
        .data(),
        amount,
    )
}

fn invoke<'info, T: ToAccountMetas + ToAccountInfos<'info>>(
    ctx: CpiContext<'_, '_, '_, 'info, T>,
    data: Vec<u8>,
    amount: u64,
) -> Result<()> {
    let mut accounts = ctx.to_account_metas(None);
    // Both typed custody layouts put root after payer, authority and funder.
    // Change only this meta: vault/wallet requirements and remaining-account
    // privileges must be retained, including any other role sharing this key.
    accounts[3].is_writable = amount != 0;
    let instruction = Instruction {
        program_id: ctx.program_id,
        accounts,
        data,
    };
    invoke_signed(&instruction, &ctx.to_account_infos(), ctx.signer_seeds).map_err(Into::into)
}
