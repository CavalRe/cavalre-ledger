//! Standalone Ledger program. The checked-in identity is for local simulation.
use anchor_lang::prelude::*;
#[cfg(feature = "mutations")]
pub mod ledger;
pub mod ledger_lib;
pub mod ledger_view;
#[cfg(feature = "mutations")]
use ledger::*;
declare_id!("HZyRps1XCM8cFqT7wZ8guNzV22EkVU4pLUBi9LNzpo97");

#[cfg(feature = "mutations")]
#[program]
pub mod cavalre_ledger_solana {
    use super::*;
    pub fn add_ledger<'info>(
        ctx: Context<'info, RegisterLedger<'info>>,
        id: Pubkey,
        name: String,
        symbol: String,
        decimals: u8,
    ) -> Result<()> {
        ledger::add_ledger(&ctx, id, name, symbol, decimals)
    }
    pub fn add_external_token<'info>(ctx: Context<'info, RegisterToken<'info>>) -> Result<()> {
        ledger::add_external_token(ctx)
    }
    pub fn add_native_sol<'info>(ctx: Context<'info, RegisterSol<'info>>) -> Result<()> {
        ledger::add_native_sol(ctx)
    }
    pub fn add_sub_account<'info>(
        ctx: Context<'info, LedgerAccounts<'info>>,
        parent: Pubkey,
        relative: Pubkey,
        name: String,
        credit: bool,
    ) -> Result<()> {
        ledger::add_account(&ctx, parent, relative, name, credit, false, true)
    }
    pub fn add_sub_account_group<'info>(
        ctx: Context<'info, LedgerAccounts<'info>>,
        parent: Pubkey,
        relative: Pubkey,
        name: String,
        credit: bool,
        implicit_allowed: bool,
    ) -> Result<()> {
        ledger::add_account(&ctx, parent, relative, name, credit, true, implicit_allowed)
    }
    pub fn add_sub_account_by_name<'info>(
        ctx: Context<'info, LedgerAccounts<'info>>,
        parent: Pubkey,
        name: String,
        credit: bool,
    ) -> Result<()> {
        let relative = ledger_lib::name_to_address(&name)
            .map_err(|_| error!(ledger_lib::LedgerError::InvalidName))?;
        add_sub_account(ctx, parent, relative, name, credit)
    }
    pub fn add_sub_account_group_by_name<'info>(
        ctx: Context<'info, LedgerAccounts<'info>>,
        parent: Pubkey,
        name: String,
        credit: bool,
        implicit_allowed: bool,
    ) -> Result<()> {
        let relative = ledger_lib::name_to_address(&name)
            .map_err(|_| error!(ledger_lib::LedgerError::InvalidName))?;
        add_sub_account_group(ctx, parent, relative, name, credit, implicit_allowed)
    }
    pub fn remove_sub_account<'info>(
        ctx: Context<'info, LedgerAccounts<'info>>,
        parent: Pubkey,
        relative: Pubkey,
    ) -> Result<()> {
        ledger::remove_account(&ctx, parent, relative, false)
    }
    pub fn remove_sub_account_group<'info>(
        ctx: Context<'info, LedgerAccounts<'info>>,
        parent: Pubkey,
        relative: Pubkey,
    ) -> Result<()> {
        ledger::remove_account(&ctx, parent, relative, true)
    }
    pub fn transfer<'info>(
        ctx: Context<'info, LedgerAccounts<'info>>,
        from_parent: Pubkey,
        from: Pubkey,
        to_parent: Pubkey,
        to: Pubkey,
        amount: u128,
    ) -> Result<()> {
        ledger::transfer(&ctx, from_parent, from, to_parent, to, amount)
    }
    pub fn wrap<'info>(
        ctx: Context<'info, MoveTokens<'info>>,
        parent: Pubkey,
        relative: Pubkey,
        amount: u64,
    ) -> Result<()> {
        ledger::move_tokens(ctx, parent, relative, amount, true)
    }
    pub fn unwrap<'info>(
        ctx: Context<'info, MoveTokens<'info>>,
        parent: Pubkey,
        relative: Pubkey,
        amount: u64,
    ) -> Result<()> {
        ledger::move_tokens(ctx, parent, relative, amount, false)
    }
    pub fn wrap_sol<'info>(
        ctx: Context<'info, MoveSol<'info>>,
        parent: Pubkey,
        relative: Pubkey,
        amount: u64,
    ) -> Result<()> {
        ledger::move_sol(ctx, parent, relative, amount, true)
    }
    pub fn unwrap_sol<'info>(
        ctx: Context<'info, MoveSol<'info>>,
        parent: Pubkey,
        relative: Pubkey,
        amount: u64,
    ) -> Result<()> {
        ledger::move_sol(ctx, parent, relative, amount, false)
    }
}
