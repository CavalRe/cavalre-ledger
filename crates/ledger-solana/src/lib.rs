//! Independent omnibus custody and hierarchical double-entry accounting.
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};
use cavalre_ledger_core::{self as ledger_core, Authorization};

pub mod hierarchy;
pub use hierarchy::*;
mod tokens;

// Simulator identity only. Deployment requires a separately selected identity.
declare_id!("DSXaqgjqGtTWfvk89dvXgii4x6EimeALFjhbxnN3mYmy");
pub const STATE_VERSION: u8 = 1;

#[program]
pub mod cavalre_ledger {
    use super::*;

    pub fn initialize_journal(ctx: Context<InitializeJournal>, id: u64) -> Result<()> {
        hierarchy::handle_initialize_journal(ctx, id)
    }

    pub fn create_node(ctx: Context<CreateNode>, relative: Pubkey, kind: NodeKind) -> Result<()> {
        hierarchy::handle_create_node(ctx, relative, kind)
    }

    pub fn close_node(ctx: Context<CloseNode>) -> Result<()> {
        hierarchy::handle_close_node(ctx)
    }

    pub fn post_journal(ctx: Context<PostJournal>, from: u8, to: u8, amount: u128) -> Result<()> {
        hierarchy::handle_post_journal(ctx, from, to, amount)
    }

    pub fn transfer_journal(
        ctx: Context<TransferHierarchy>,
        from: u8,
        to: u8,
        amount: u128,
    ) -> Result<()> {
        hierarchy::handle_transfer_journal(ctx, from, to, amount)
    }

    pub fn transfer_nested(
        ctx: Context<TransferHierarchy>,
        from: u8,
        to: u8,
        amount: u64,
    ) -> Result<()> {
        hierarchy::handle_transfer_nested(ctx, from, to, amount)
    }

    pub fn initialize_ledger(ctx: Context<InitializeLedger>, id: u64) -> Result<()> {
        ledger_core::initialize_namespace(
            ctx.accounts.ledger.key().to_bytes(),
            ctx.accounts.authority.key().to_bytes(),
            Authorization::from_verified_signers(&[ctx.accounts.authority.key().to_bytes()]),
        )
        .map_err(hierarchy::core_error)?;
        ctx.accounts.ledger.set_inner(Ledger {
            authority: ctx.accounts.authority.key(),
            id,
            bump: ctx.bumps.ledger,
            version: STATE_VERSION,
        });
        Ok(())
    }

    pub fn register_asset(ctx: Context<RegisterAsset>) -> Result<()> {
        ledger_core::create_root(
            &ctx.accounts.ledger.core(ctx.accounts.ledger.key()),
            ctx.accounts.asset.key().to_bytes(),
            ledger_core::RootKind::Asset,
            Authorization::from_verified_signers(&[ctx.accounts.authority.key().to_bytes()]),
        )
        .map_err(hierarchy::core_error)?;
        tokens::validate_mint(&ctx.accounts.mint.to_account_info())?;
        tokens::initialize_vault(ctx.accounts)?;
        ctx.accounts.asset.set_inner(Asset {
            ledger: ctx.accounts.ledger.key(),
            mint: ctx.accounts.mint.key(),
            total_claims: 0,
            bump: ctx.bumps.asset,
            vault_bump: ctx.bumps.vault,
            version: STATE_VERSION,
        });
        Ok(())
    }

    pub fn open_position(ctx: Context<OpenPosition>) -> Result<()> {
        ledger_core::open_position(
            &ctx.accounts.asset.core(ctx.accounts.asset.key()),
            ctx.accounts.position.key().to_bytes(),
            ctx.accounts.owner.key().to_bytes(),
            Authorization::from_verified_signers(&[ctx.accounts.owner.key().to_bytes()]),
        )
        .map_err(hierarchy::core_error)?;
        ctx.accounts.position.set_inner(Position {
            asset: ctx.accounts.asset.key(),
            owner: ctx.accounts.owner.key(),
            balance: 0,
            bump: ctx.bumps.position,
            version: STATE_VERSION,
        });
        Ok(())
    }

    pub fn deposit<'info>(ctx: Context<'info, MoveTokens<'info>>, amount: u64) -> Result<()> {
        let plan = ledger_core::prepare_deposit(
            &ctx.accounts.asset.core(ctx.accounts.asset.key()),
            &ctx.accounts.position.core(ctx.accounts.position.key()),
            amount.into(),
            ctx.accounts.token_balances(),
            Authorization::from_verified_signers(&[ctx.accounts.owner.key().to_bytes()]),
        )
        .map_err(hierarchy::core_error)?;
        let (balance, total_claims) = custody_amounts(&plan)?;
        tokens::transfer(ctx.accounts, ctx.remaining_accounts, amount, true, &[])?;
        ctx.accounts.vault.reload()?;
        ctx.accounts.wallet.reload()?;
        plan.verify_settlement(ctx.accounts.token_balances())
            .map_err(hierarchy::core_error)?;
        // Mint the internal claim only after observing exact collateral receipt.
        ctx.accounts.position.balance = balance;
        ctx.accounts.asset.total_claims = total_claims;
        emit!(CustodyChanged {
            asset: ctx.accounts.asset.key(),
            owner: ctx.accounts.owner.key(),
            deposit: true,
            amount,
            balance,
            total_claims,
        });
        Ok(())
    }

    pub fn withdraw<'info>(ctx: Context<'info, MoveTokens<'info>>, amount: u64) -> Result<()> {
        let plan = ledger_core::prepare_withdrawal(
            &ctx.accounts.asset.core(ctx.accounts.asset.key()),
            &ctx.accounts.position.core(ctx.accounts.position.key()),
            amount.into(),
            ctx.accounts.token_balances(),
            Authorization::from_verified_signers(&[ctx.accounts.owner.key().to_bytes()]),
        )
        .map_err(hierarchy::core_error)?;
        let (balance, total_claims) = custody_amounts(&plan)?;
        let ledger = ctx.accounts.asset.ledger;
        let mint = ctx.accounts.asset.mint;
        let bump = [ctx.accounts.asset.bump];
        let seeds: &[&[u8]] = &[b"asset", ledger.as_ref(), mint.as_ref(), &bump];

        // Debit before CPI; Solana rolls the entire transaction back on failure.
        ctx.accounts.position.balance = balance;
        ctx.accounts.asset.total_claims = total_claims;
        tokens::transfer(
            ctx.accounts,
            ctx.remaining_accounts,
            amount,
            false,
            &[seeds],
        )?;
        ctx.accounts.vault.reload()?;
        ctx.accounts.wallet.reload()?;
        plan.verify_settlement(ctx.accounts.token_balances())
            .map_err(hierarchy::core_error)?;
        emit!(CustodyChanged {
            asset: ctx.accounts.asset.key(),
            owner: ctx.accounts.owner.key(),
            deposit: false,
            amount,
            balance,
            total_claims,
        });
        Ok(())
    }

    pub fn close_position(ctx: Context<ClosePosition>) -> Result<()> {
        ctx.accounts
            .position
            .core(ctx.accounts.position.key())
            .check_close(Authorization::from_verified_signers(&[ctx
                .accounts
                .owner
                .key()
                .to_bytes()]))
            .map_err(hierarchy::core_error)?;
        // Account constraints authenticate the position and refund rent to owner.
        Ok(())
    }

    pub fn transfer_claims(ctx: Context<TransferClaims>, amount: u64) -> Result<()> {
        let root = ctx.accounts.asset.core(ctx.accounts.asset.key());
        let from = ctx.accounts.source.core(ctx.accounts.source.key());
        let to = ctx
            .accounts
            .destination
            .core(ctx.accounts.destination.key());
        let records = [from, to];
        // A self-transfer has one authenticated record, not duplicate snapshots.
        let (records, destination) = if from.node.id == to.node.id {
            (&records[..1], 0)
        } else {
            (&records[..], 1)
        };
        let plan = ledger_core::plan_posting(
            &root,
            records,
            0,
            destination,
            amount.into(),
            ledger_core::Operation::TransferClaims,
            Authorization::from_verified_signers(&[ctx.accounts.owner.key().to_bytes()]),
        )
        .map_err(hierarchy::core_error)?;
        // There are only two possible writes. Stage both before applying either.
        let mut source = ctx.accounts.source.balance;
        let mut destination = ctx.accounts.destination.balance;
        for change in plan.changes() {
            let balance =
                u64::try_from(change.after.debit).map_err(|_| CustodyError::ArithmeticOverflow)?;
            if change.id == from.node.id {
                source = balance;
            }
            if change.id == to.node.id {
                destination = balance;
            }
        }
        // A self-transfer plan is empty after checking the sender's balance;
        // both aliasing Anchor values remain identical on serialization.
        ctx.accounts.source.balance = source;
        ctx.accounts.destination.balance = destination;
        emit!(ClaimsTransferred {
            asset: ctx.accounts.asset.key(),
            from: ctx.accounts.owner.key(),
            to: ctx.accounts.destination.owner,
            amount,
        });
        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(id: u64)]
pub struct InitializeLedger<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(init, payer = authority, space = 8 + Ledger::INIT_SPACE,
        seeds = [b"ledger", authority.key().as_ref(), &id.to_le_bytes()], bump)]
    pub ledger: Account<'info, Ledger>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RegisterAsset<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(seeds = [b"ledger", ledger.authority.as_ref(), &ledger.id.to_le_bytes()],
        bump = ledger.bump, has_one = authority,
        constraint = ledger.version == STATE_VERSION @ CustodyError::UnsupportedVersion)]
    pub ledger: Account<'info, Ledger>,
    #[account(owner = token_program.key())]
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(init, payer = authority, space = 8 + Asset::INIT_SPACE,
        seeds = [b"asset", ledger.key().as_ref(), mint.key().as_ref()], bump)]
    pub asset: Account<'info, Asset>,
    /// CHECK: initialized here with the authenticated mint, token program and Asset authority.
    #[account(init, payer = authority, seeds = [b"vault", asset.key().as_ref()], bump,
        space = tokens::vault_space(&mint.to_account_info(), token_program.key())?,
        owner = token_program.key())]
    pub vault: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct OpenPosition<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(seeds = [b"asset", asset.ledger.as_ref(), asset.mint.as_ref()],
        bump = asset.bump,
        constraint = asset.version == STATE_VERSION @ CustodyError::UnsupportedVersion)]
    pub asset: Account<'info, Asset>,
    #[account(init, payer = owner, space = 8 + Position::INIT_SPACE,
        seeds = [b"position", asset.key().as_ref(), owner.key().as_ref()], bump)]
    pub position: Account<'info, Position>,
    pub system_program: Program<'info, System>,
}

/// Both directions use the same fully constrained custody boundary. The ledger
/// namespace is immutable in Asset; hot paths do not need a global ledger lock.
#[derive(Accounts)]
pub struct MoveTokens<'info> {
    pub owner: Signer<'info>,
    #[account(mut, seeds = [b"asset", asset.ledger.as_ref(), mint.key().as_ref()],
        bump = asset.bump, has_one = mint,
        constraint = asset.version == STATE_VERSION @ CustodyError::UnsupportedVersion)]
    pub asset: Account<'info, Asset>,
    #[account(mut, seeds = [b"position", asset.key().as_ref(), owner.key().as_ref()],
        bump = position.bump, has_one = asset, has_one = owner,
        constraint = position.version == STATE_VERSION @ CustodyError::UnsupportedVersion)]
    pub position: Account<'info, Position>,
    #[account(owner = token_program.key())]
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut, seeds = [b"vault", asset.key().as_ref()], bump = asset.vault_bump,
        token::mint = mint, token::authority = asset, token::token_program = token_program)]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut, token::mint = mint, token::authority = owner,
        token::token_program = token_program,
        constraint = wallet.key() != vault.key() @ CustodyError::AliasedVault)]
    pub wallet: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct ClosePosition<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(mut, close = owner,
        seeds = [b"position", position.asset.as_ref(), owner.key().as_ref()],
        bump = position.bump, has_one = owner,
        constraint = position.version == STATE_VERSION @ CustodyError::UnsupportedVersion,
        constraint = position.balance == 0 @ CustodyError::NonemptyPosition)]
    pub position: Account<'info, Position>,
}

#[account]
#[derive(InitSpace)]
pub struct Ledger {
    pub authority: Pubkey,
    pub id: u64,
    pub bump: u8,
    pub version: u8,
}

#[derive(Accounts)]
pub struct TransferClaims<'info> {
    pub owner: Signer<'info>,
    #[account(seeds = [b"asset", asset.ledger.as_ref(), asset.mint.as_ref()],
        bump = asset.bump,
        constraint = asset.version == STATE_VERSION @ CustodyError::UnsupportedVersion)]
    pub asset: Account<'info, Asset>,
    #[account(mut, seeds = [b"position", asset.key().as_ref(), owner.key().as_ref()],
        bump = source.bump, has_one = asset, has_one = owner,
        constraint = source.version == STATE_VERSION @ CustodyError::UnsupportedVersion)]
    pub source: Account<'info, Position>,
    // The sole deliberate alias: self-transfer is validated then leaves state
    // unchanged. All other mutable typed accounts retain duplicate protection.
    #[account(mut, dup,
        seeds = [b"position", asset.key().as_ref(), destination.owner.as_ref()],
        bump = destination.bump, has_one = asset,
        constraint = destination.version == STATE_VERSION @ CustodyError::UnsupportedVersion)]
    pub destination: Account<'info, Position>,
}

#[account]
#[derive(InitSpace)]
pub struct Asset {
    pub ledger: Pubkey,
    pub mint: Pubkey,
    /// Aggregate outstanding custody claims, not a cached vault balance.
    pub total_claims: u64,
    pub bump: u8,
    pub vault_bump: u8,
    pub version: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Position {
    pub asset: Pubkey,
    pub owner: Pubkey,
    pub balance: u64,
    pub bump: u8,
    pub version: u8,
}

#[event]
pub struct CustodyChanged {
    pub asset: Pubkey,
    pub owner: Pubkey,
    pub deposit: bool,
    pub amount: u64,
    pub balance: u64,
    pub total_claims: u64,
}

#[event]
pub struct ClaimsTransferred {
    pub asset: Pubkey,
    pub from: Pubkey,
    pub to: Pubkey,
    pub amount: u64,
}

#[error_code]
pub enum CustodyError {
    #[msg("Account layout version is not supported")]
    UnsupportedVersion,
    #[msg("Checked custody arithmetic overflowed or aggregate state is inconsistent")]
    ArithmeticOverflow,
    #[msg("The position cannot cover this withdrawal")]
    InsufficientBalance,
    #[msg("Vault collateral does not cover all outstanding claims")]
    Undercollateralized,
    #[msg("The token transfer did not produce the exact expected balance change")]
    UnexpectedTokenDelta,
    #[msg("A vault cannot be used as the holder's wallet")]
    AliasedVault,
    #[msg("Withdraw the position's entire balance before closing it")]
    NonemptyPosition,
    #[msg("The posting does not satisfy the accounting topology")]
    InvalidPosting,
    #[msg("The signer does not control this node or its parent")]
    NodeAuthority,
    #[msg("Invalid node owner, identity, layout, version or root membership")]
    InvalidNode,
    #[msg("Supply exactly the unique union of both complete paths, excluding the root")]
    InvalidAccounts,
    #[msg("Every changed account must be writable")]
    ReadonlyPosting,
    #[msg("A node must have no children and zero gross balances before closing")]
    NonemptyNode,
    #[msg("This instruction or node kind is not supported for this root type")]
    UnsupportedRoot,
    #[msg("Hierarchy exceeds the supported path bound")]
    DepthLimit,
    #[msg("Token extension is outside the exact-custody support policy")]
    UnsupportedTokenExtension,
}

impl Ledger {
    pub(crate) fn core(&self, key: Pubkey) -> ledger_core::Namespace {
        ledger_core::Namespace {
            id: key.to_bytes(),
            authority: self.authority.to_bytes(),
        }
    }
}
impl Asset {
    pub(crate) fn core(&self, key: Pubkey) -> ledger_core::Root {
        ledger_core::Root {
            id: key.to_bytes(),
            namespace: self.ledger.to_bytes(),
            kind: ledger_core::RootKind::Asset,
            gross: self.total_claims.into(),
        }
    }
}
impl Position {
    pub(crate) fn core(&self, key: Pubkey) -> ledger_core::Account {
        ledger_core::Account {
            node: ledger_core::Node {
                id: key.to_bytes(),
                parent: Some(self.asset.to_bytes()),
                kind: ledger_core::Kind::DebitLeaf,
                balances: ledger_core::Balances {
                    debit: self.balance.into(),
                    credit: 0,
                },
            },
            root: self.asset.to_bytes(),
            controller: self.owner.to_bytes(),
            children: 0,
            depth: 1,
        }
    }
}
impl MoveTokens<'_> {
    fn token_balances(&self) -> ledger_core::TokenBalances {
        ledger_core::TokenBalances {
            vault: self.vault.amount.into(),
            wallet: self.wallet.amount.into(),
        }
    }
}
fn custody_amounts(plan: &ledger_core::CustodyPlan) -> Result<(u64, u64)> {
    Ok((
        u64::try_from(plan.position().after.debit).map_err(|_| CustodyError::ArithmeticOverflow)?,
        u64::try_from(plan.root().after.debit).map_err(|_| CustodyError::ArithmeticOverflow)?,
    ))
}
