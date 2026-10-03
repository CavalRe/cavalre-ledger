//! Authenticated, bounded ancestry for controller-authorized accounting.
//!
//! Clients supply an unordered union of account addresses, never snapshots or
//! parent claims. Each changed record is serialized once. Internal journal
//! credits have no SPL redemption route; both endpoint controllers consent.
use crate::{Asset, CustodyError as E, Ledger, Position, STATE_VERSION};
use anchor_lang::prelude::*;
use cavalre_ledger_core::{self as ledger_core, Authorization, Balances, Change, Kind, Node};

pub const NODE_VERSION: u8 = 1;

pub use ledger_core::MAX_RECORDS;

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    DebitLeaf,
    CreditLeaf,
    DebitGroup,
    CreditGroup,
}
impl NodeKind {
    pub fn kernel(self) -> Kind {
        match self {
            Self::DebitLeaf => Kind::DebitLeaf,
            Self::CreditLeaf => Kind::CreditLeaf,
            Self::DebitGroup => Kind::DebitGroup,
            Self::CreditGroup => Kind::CreditGroup,
        }
    }
}

/// Ordinary internal unit of account, with no mint, vault or redemption route.
/// Root D and C are equal and represented by one authoritative gross value.
#[account]
#[derive(InitSpace)]
pub struct Journal {
    pub ledger: Pubkey,
    pub id: u64,
    pub gross: u128,
    pub bump: u8,
    pub version: u8,
}

/// One independently lockable economic node, not one account per mapping field.
/// Parent, kind, identity and controller are immutable for its funded lifetime.
#[account]
#[derive(InitSpace)]
pub struct LedgerNode {
    pub root: Pubkey,
    pub parent: Pubkey,
    pub relative: Pubkey,
    pub controller: Pubkey,
    pub rent_payer: Pubkey,
    pub debit: u128,
    pub credit: u128,
    pub children: u32,
    pub depth: u8,
    pub kind: NodeKind,
    pub bump: u8,
    pub version: u8,
}

#[derive(Accounts)]
#[instruction(id: u64)]
pub struct InitializeJournal<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(seeds = [b"ledger", ledger.authority.as_ref(), &ledger.id.to_le_bytes()],
        bump = ledger.bump, has_one = authority,
        constraint = ledger.version == STATE_VERSION @ E::UnsupportedVersion)]
    pub ledger: Account<'info, Ledger>,
    #[account(init, payer = authority, space = 8 + Journal::INIT_SPACE,
        seeds = [b"journal", ledger.key().as_ref(), &id.to_le_bytes()], bump)]
    pub journal: Account<'info, Journal>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(relative: Pubkey)]
pub struct CreateNode<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// A distinct controller may be a PDA signing through CPI.
    pub controller: Signer<'info>,
    pub parent_authority: Signer<'info>,
    #[account(seeds = [b"ledger", ledger.authority.as_ref(), &ledger.id.to_le_bytes()],
        bump = ledger.bump,
        constraint = ledger.version == STATE_VERSION @ E::UnsupportedVersion)]
    pub ledger: Account<'info, Ledger>,
    /// CHECK: load_root checks owner, discriminator, length, version and PDA.
    pub root: UncheckedAccount<'info>,
    /// CHECK: checked against root or a validated group; writable only for a group.
    pub parent: UncheckedAccount<'info>,
    #[account(init, payer = payer, space = 8 + LedgerNode::INIT_SPACE,
        seeds = [b"node", root.key().as_ref(), parent.key().as_ref(), relative.as_ref()], bump)]
    pub node: Account<'info, LedgerNode>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CloseNode<'info> {
    pub controller: Signer<'info>,
    /// CHECK: immutable creation payer, checked by has_one; receives rent only.
    #[account(mut)]
    pub rent_payer: UncheckedAccount<'info>,
    /// CHECK: load_root validates the identity and type.
    pub root: UncheckedAccount<'info>,
    /// CHECK: validated immutable parent; group child count decremented manually.
    pub parent: UncheckedAccount<'info>,
    #[account(mut, close = rent_payer, has_one = controller, has_one = rent_payer,
        seeds = [b"node", root.key().as_ref(), parent.key().as_ref(), node.relative.as_ref()],
        bump = node.bump, has_one = root, has_one = parent,
        constraint = node.version == NODE_VERSION @ E::UnsupportedVersion,
        constraint = node.debit == 0 && node.credit == 0 && node.children == 0 @ E::NonemptyNode)]
    pub node: Account<'info, LedgerNode>,
}

#[derive(Accounts)]
pub struct PostJournal<'info> {
    pub from_authority: Signer<'info>,
    pub to_authority: Signer<'info>,
    /// CHECK: authenticated Journal only; manually serialized iff its gross changes.
    pub root: UncheckedAccount<'info>,
    // Remaining accounts: unique union of both endpoint-to-root paths, no root.
}

#[derive(Accounts)]
pub struct TransferHierarchy<'info> {
    pub owner: Signer<'info>,
    /// CHECK: authenticated root of the type required by the chosen instruction.
    pub root: UncheckedAccount<'info>,
}

pub(crate) fn handle_initialize_journal(ctx: Context<InitializeJournal>, id: u64) -> Result<()> {
    ledger_core::create_root(
        &ctx.accounts.ledger.core(ctx.accounts.ledger.key()),
        ctx.accounts.journal.key().to_bytes(),
        ledger_core::RootKind::Journal,
        Authorization::from_verified_signers(&[ctx.accounts.authority.key().to_bytes()]),
    )
    .map_err(core_error)?;
    ctx.accounts.journal.set_inner(Journal {
        ledger: ctx.accounts.ledger.key(),
        id,
        gross: 0,
        bump: ctx.bumps.journal,
        version: STATE_VERSION,
    });
    Ok(())
}

pub(crate) fn handle_create_node(
    ctx: Context<CreateNode>,
    relative: Pubkey,
    kind: NodeKind,
) -> Result<()> {
    let root = load_root(&ctx.accounts.root)?;
    let mut parent = if ctx.accounts.parent.key() == ctx.accounts.root.key() {
        None
    } else {
        Some(load_node(
            &ctx.accounts.parent,
            ctx.accounts.root.key(),
            root.is_asset(),
        )?)
    };
    let parent_view = parent.as_ref().map(|n| n.core(ctx.accounts.parent.key()));
    let signers = [
        ctx.accounts.controller.key().to_bytes(),
        ctx.accounts.parent_authority.key().to_bytes(),
    ];
    let creation = ledger_core::create_node(
        &root.core(ctx.accounts.root.key()),
        &ctx.accounts.ledger.core(ctx.accounts.ledger.key()),
        parent_view.as_ref(),
        ledger_core::NewNode {
            id: ctx.accounts.node.key().to_bytes(),
            controller: signers[0],
            kind: kind.kernel(),
        },
        Authorization::from_verified_signers(&signers),
    )
    .map_err(core_error)?;
    if let (Some(parent), Some(change)) = (&mut parent, creation.parent) {
        parent.children = change.after;
        write(&ctx.accounts.parent, parent)?;
    }
    ctx.accounts.node.set_inner(LedgerNode {
        root: ctx.accounts.root.key(),
        parent: ctx.accounts.parent.key(),
        relative,
        controller: ctx.accounts.controller.key(),
        rent_payer: ctx.accounts.payer.key(),
        debit: 0,
        credit: 0,
        children: 0,
        depth: creation.account.depth,
        kind,
        bump: ctx.bumps.node,
        version: NODE_VERSION,
    });
    Ok(())
}

pub(crate) fn handle_close_node(ctx: Context<CloseNode>) -> Result<()> {
    let root = load_root(&ctx.accounts.root)?;
    let mut parent = if ctx.accounts.parent.key() == ctx.accounts.root.key() {
        None
    } else {
        Some(load_node(
            &ctx.accounts.parent,
            ctx.accounts.root.key(),
            root.is_asset(),
        )?)
    };
    let parent_view = parent.as_ref().map(|n| n.core(ctx.accounts.parent.key()));
    let change = ledger_core::close_node(
        &root.core(ctx.accounts.root.key()),
        &ctx.accounts.node.core(ctx.accounts.node.key()),
        parent_view.as_ref(),
        Authorization::from_verified_signers(&[ctx.accounts.controller.key().to_bytes()]),
    )
    .map_err(core_error)?;
    if let (Some(parent), Some(change)) = (&mut parent, change) {
        parent.children = change.after;
        write(&ctx.accounts.parent, parent)?;
    }
    // Anchor closes after success and refunds the immutable creation payer.
    Ok(())
}

pub(crate) fn handle_post_journal(
    ctx: Context<PostJournal>,
    from: u8,
    to: u8,
    amount: u128,
) -> Result<()> {
    post(
        &ctx.accounts.root,
        ctx.remaining_accounts,
        from,
        to,
        amount,
        ledger_core::Operation::PostJournal,
        &[
            ctx.accounts.from_authority.key().to_bytes(),
            ctx.accounts.to_authority.key().to_bytes(),
        ],
    )
}

pub(crate) fn handle_transfer_journal(
    ctx: Context<TransferHierarchy>,
    from: u8,
    to: u8,
    amount: u128,
) -> Result<()> {
    post(
        &ctx.accounts.root,
        ctx.remaining_accounts,
        from,
        to,
        amount,
        ledger_core::Operation::TransferJournal,
        &[ctx.accounts.owner.key().to_bytes()],
    )
}

pub(crate) fn handle_transfer_nested(
    ctx: Context<TransferHierarchy>,
    from: u8,
    to: u8,
    amount: u64,
) -> Result<()> {
    post(
        &ctx.accounts.root,
        ctx.remaining_accounts,
        from,
        to,
        amount.into(),
        ledger_core::Operation::TransferClaims,
        &[ctx.accounts.owner.key().to_bytes()],
    )
}

fn post(
    root_info: &AccountInfo,
    infos: &[AccountInfo],
    from: u8,
    to: u8,
    amount: u128,
    operation: ledger_core::Operation,
    signers: &[ledger_core::Id],
) -> Result<()> {
    let root = load_root(root_info)?;
    let root_view = root.core(*root_info.key);
    operation
        .validate_root(root_view.kind)
        .map_err(core_error)?;
    let records = load_records(infos, *root_info.key, root.is_asset())?;
    let views: Vec<_> = records.iter().map(Record::core).collect();
    let plan = ledger_core::plan_posting(
        &root_view,
        &views,
        from.into(),
        to.into(),
        amount,
        operation,
        Authorization::from_verified_signers(signers),
    )
    .map_err(core_error)?;
    commit(root_info, root, infos, records, plan.changes())?;
    emit_posting(*root_info.key, &plan, amount);
    Ok(())
}

pub(crate) fn core_error(e: ledger_core::Error) -> anchor_lang::error::Error {
    match e {
        ledger_core::Error::Unauthorized => E::NodeAuthority,
        ledger_core::Error::InvalidNode => E::InvalidNode,
        ledger_core::Error::InvalidAccounts => E::InvalidAccounts,
        ledger_core::Error::UnsupportedRoot => E::UnsupportedRoot,
        ledger_core::Error::NonemptyNode => E::NonemptyNode,
        ledger_core::Error::DepthLimit => E::DepthLimit,
        ledger_core::Error::Overflow => E::ArithmeticOverflow,
        ledger_core::Error::InsufficientBalance => E::InsufficientBalance,
        ledger_core::Error::Undercollateralized => E::Undercollateralized,
        ledger_core::Error::UnexpectedTokenDelta => E::UnexpectedTokenDelta,
        ledger_core::Error::Accounting(_) => E::InvalidPosting,
    }
    .into()
}

pub(crate) enum Root {
    Asset(Asset),
    Journal(Journal),
}
impl Root {
    pub(crate) fn is_asset(&self) -> bool {
        matches!(self, Self::Asset(_))
    }
    pub(crate) fn ledger(&self) -> Pubkey {
        match self {
            Self::Asset(a) => a.ledger,
            Self::Journal(j) => j.ledger,
        }
    }
    pub(crate) fn core(&self, key: Pubkey) -> ledger_core::Root {
        ledger_core::Root {
            id: key.to_bytes(),
            namespace: self.ledger().to_bytes(),
            kind: if self.is_asset() {
                ledger_core::RootKind::Asset
            } else {
                ledger_core::RootKind::Journal
            },
            gross: match self {
                Self::Asset(a) => a.total_claims.into(),
                Self::Journal(j) => j.gross,
            },
        }
    }
}

fn owner(info: &AccountInfo) -> Result<()> {
    require!(info.owner == &crate::ID && !info.executable, E::InvalidNode);
    Ok(())
}
pub(crate) fn identity(info: &AccountInfo, seeds: &[&[u8]]) -> Result<()> {
    let expected = Pubkey::create_program_address(seeds, &crate::ID).map_err(|_| E::InvalidNode)?;
    require_keys_eq!(expected, *info.key, E::InvalidNode);
    Ok(())
}
pub(crate) fn read<T: AccountDeserialize + Space>(info: &AccountInfo) -> Result<T> {
    owner(info)?;
    require!(info.data_len() == 8 + T::INIT_SPACE, E::InvalidNode);
    T::try_deserialize(&mut info.try_borrow_data()?.as_ref()).map_err(|_| E::InvalidNode.into())
}
pub(crate) fn write<T: AccountSerialize>(info: &AccountInfo, value: &T) -> Result<()> {
    require!(info.is_writable, E::ReadonlyPosting);
    value.try_serialize(&mut info.try_borrow_mut_data()?.as_mut())
}
pub(crate) fn load_root(info: &AccountInfo) -> Result<Root> {
    owner(info)?;
    let is_asset = info.try_borrow_data()?.starts_with(Asset::DISCRIMINATOR);
    if is_asset {
        let a: Asset = read(info)?;
        require!(a.version == STATE_VERSION, E::UnsupportedVersion);
        identity(
            info,
            &[b"asset", a.ledger.as_ref(), a.mint.as_ref(), &[a.bump]],
        )?;
        Ok(Root::Asset(a))
    } else {
        let j: Journal = read(info)?;
        require!(j.version == STATE_VERSION, E::UnsupportedVersion);
        identity(
            info,
            &[
                b"journal",
                j.ledger.as_ref(),
                &j.id.to_le_bytes(),
                &[j.bump],
            ],
        )?;
        Ok(Root::Journal(j))
    }
}
pub(crate) fn load_node(info: &AccountInfo, root: Pubkey, asset: bool) -> Result<LedgerNode> {
    let n: LedgerNode = read(info)?;
    require!(n.version == NODE_VERSION, E::UnsupportedVersion);
    require_keys_eq!(n.root, root, E::InvalidNode);
    identity(
        info,
        &[
            b"node",
            root.as_ref(),
            n.parent.as_ref(),
            n.relative.as_ref(),
            &[n.bump],
        ],
    )?;
    // Solana's native-token amount bound is narrower than the portable u128 core.
    require!(
        !asset || n.debit <= u128::from(u64::MAX),
        E::UnsupportedRoot
    );
    Ok(n)
}

pub(crate) enum State {
    Node(LedgerNode),
    Position(Position),
}
pub(crate) struct Record {
    pub(crate) key: Pubkey,
    pub(crate) state: State,
}
impl LedgerNode {
    fn core(&self, key: Pubkey) -> ledger_core::Account {
        ledger_core::Account {
            node: Node {
                id: key.to_bytes(),
                parent: Some(self.parent.to_bytes()),
                kind: self.kind.kernel(),
                balances: Balances {
                    debit: self.debit,
                    credit: self.credit,
                },
            },
            root: self.root.to_bytes(),
            controller: self.controller.to_bytes(),
            children: self.children,
            depth: self.depth,
        }
    }
}
impl Record {
    fn core(&self) -> ledger_core::Account {
        match &self.state {
            State::Node(n) => n.core(self.key),
            State::Position(p) => p.core(self.key),
        }
    }
}
pub(crate) fn load_records(
    infos: &[AccountInfo],
    root: Pubkey,
    asset: bool,
) -> Result<Vec<Record>> {
    require!(
        !infos.is_empty() && infos.len() <= MAX_RECORDS,
        E::InvalidAccounts
    );
    let mut records: Vec<Record> = Vec::with_capacity(infos.len());
    for info in infos {
        require!(
            *info.key != root && !records.iter().any(|n| n.key == *info.key),
            E::InvalidAccounts
        );
        owner(info)?;
        let direct = asset && info.try_borrow_data()?.starts_with(Position::DISCRIMINATOR);
        let state = if direct {
            let p: Position = read(info)?;
            require!(p.version == STATE_VERSION, E::UnsupportedVersion);
            require_keys_eq!(p.asset, root, E::InvalidNode);
            identity(
                info,
                &[b"position", root.as_ref(), p.owner.as_ref(), &[p.bump]],
            )?;
            State::Position(p)
        } else {
            State::Node(load_node(info, root, asset)?)
        };
        records.push(Record {
            key: *info.key,
            state,
        });
    }
    Ok(records)
}

pub(crate) fn commit(
    root_info: &AccountInfo,
    mut root: Root,
    infos: &[AccountInfo],
    mut records: Vec<Record>,
    plan: &[Change],
) -> Result<()> {
    // Stage and check EVERY change before the first serialization. No duplicate
    // deserialized aliases, partial write plans, or CPI during commit.
    let mut root_changed = false;
    for change in plan {
        if change.id == root_info.key.to_bytes() {
            require!(root_info.is_writable, E::ReadonlyPosting);
            require!(change.after.debit == change.after.credit, E::InvalidPosting);
            match &mut root {
                Root::Journal(j) => j.gross = change.after.debit,
                Root::Asset(_) => return err!(E::UnsupportedRoot), // nested routing cannot issue claims
            }
            root_changed = true;
        } else {
            let index = records
                .iter()
                .position(|r| r.key.to_bytes() == change.id)
                .ok_or(E::InvalidAccounts)?;
            require!(infos[index].is_writable, E::ReadonlyPosting);
            match &mut records[index].state {
                State::Node(n) => {
                    if root.is_asset() {
                        require!(
                            change.after.credit == 0 && change.after.debit <= u128::from(u64::MAX),
                            E::ArithmeticOverflow
                        );
                    }
                    n.debit = change.after.debit;
                    n.credit = change.after.credit;
                }
                State::Position(p) => {
                    require!(change.after.credit == 0, E::InvalidPosting);
                    p.balance =
                        u64::try_from(change.after.debit).map_err(|_| E::ArithmeticOverflow)?;
                }
            }
        }
    }
    for (record, info) in records.iter().zip(infos) {
        if plan.iter().any(|c| c.id == record.key.to_bytes()) {
            match &record.state {
                State::Node(n) => write(info, n)?,
                State::Position(p) => write(info, p)?,
            }
        }
    }
    if root_changed {
        match root {
            Root::Journal(j) => write(root_info, &j)?,
            _ => return err!(E::UnsupportedRoot),
        }
    }
    Ok(())
}

#[event]
pub struct HierarchyPosted {
    pub root: Pubkey,
    pub from: Pubkey,
    pub to: Pubkey,
    pub from_custody: Pubkey,
    pub to_custody: Pubkey,
    pub from_credit: bool,
    pub to_credit: bool,
    pub amount: u128,
}
pub(crate) fn emit_posting(root: Pubkey, plan: &ledger_core::Posting, amount: u128) {
    emit!(HierarchyPosted {
        root,
        from: Pubkey::new_from_array(plan.from.id),
        to: Pubkey::new_from_array(plan.to.id),
        from_custody: Pubkey::new_from_array(plan.from.custody),
        to_custody: Pubkey::new_from_array(plan.to.custody),
        from_credit: plan.from.credit,
        to_credit: plan.to.credit,
        amount,
    });
}
