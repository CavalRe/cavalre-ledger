//! Program operations corresponding to Ledger.sol. Shared posting rules remain
//! in LedgerLib; this module authenticates storage/signers and commits atomically.
use crate::ledger_lib::{to_address, PdaAddresses};
use anchor_lang::{prelude::*, system_program};
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};
use cavalre_ledger_core::ledger_lib as core;

pub const SOURCE: Pubkey = Pubkey::new_from_array([83; 32]);
const MAGIC: &[u8; 8] = b"CVLEDG01";
const SPACE: usize = 512;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub root: Pubkey,
    pub parent: Pubkey,
    pub relative: Pubkey,
    pub custodian: Pubkey,
    pub kind: u8,
    pub token_kind: u8,
    pub depth: u8,
    pub registered: bool,
    pub implicit_allowed: bool,
    pub children: u32,
    pub debit: u128,
    pub credit: u128,
    pub name: String,
    // Root-only identity: scope zero for an external mint, authority for internal units.
    pub scope: Pubkey,
    pub identifier: Pubkey,
    pub bump: u8,
}
impl Record {
    fn flags(&self) -> core::Flags<Pubkey> {
        core::Flags {
            parent: self.parent,
            account_kind: match self.kind {
                0 => core::AccountKind::DebitGroup,
                1 => core::AccountKind::CreditGroup,
                2 => core::AccountKind::DebitLedger,
                _ => core::AccountKind::CreditLedger,
            },
            token_kind: if self.depth == 2 {
                if self.scope == Pubkey::default() {
                    core::TokenKind::External
                } else {
                    core::TokenKind::Internal
                }
            } else {
                core::TokenKind::Unregistered
            },
            depth: self.depth,
        }
    }
}

#[derive(Accounts)]
pub struct LedgerAccounts<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    /// CHECK: root identity, ownership and data authenticated by load/initialize.
    #[account(mut)]
    pub root: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    // Remaining: endpoint records, parents, custody ancestors and changed ancestors.
    // New endpoint/Source PDAs must be supplied writable for allocation.
}
#[derive(Accounts)]
pub struct RegisterToken<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: derived from the mint, authenticated and initialized in the handler.
    #[account(mut)]
    pub root: UncheckedAccount<'info>,
    pub mint: Account<'info, Mint>,
    #[account(init, payer=payer, seeds=[b"vault", root.key().as_ref()], bump,
        token::mint=mint, token::authority=root)]
    pub vault: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct MoveTokens<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    // Deposit payer authority. On withdrawal this signature confers no additional rights.
    pub funding_authority: Signer<'info>,
    /// CHECK: authenticated root record and signer seeds in handler.
    #[account(mut)]
    pub root: UncheckedAccount<'info>,
    pub mint: Account<'info, Mint>,
    #[account(mut, seeds=[b"vault",root.key().as_ref()],bump,
        token::mint=mint,token::authority=root)]
    pub vault: Account<'info, TokenAccount>,
    #[account(mut, token::mint=mint, constraint=wallet.key()!=vault.key() @ LedgerError::InvalidAccount)]
    pub wallet: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[error_code]
pub enum LedgerError {
    InvalidAccount,
    Unauthorized,
    InvalidKind,
    Nonempty,
    MetadataConflict,
    InvalidName,
    MissingAccount,
    Accounting,
    UnsupportedToken,
    Settlement,
    Undercollateralized,
}

struct State {
    records: Vec<(Pubkey, Record)>,
}
impl State {
    fn get(&self, key: &Pubkey) -> Result<&Record> {
        self.records
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .ok_or(error!(LedgerError::MissingAccount))
    }
    fn get_mut(&mut self, key: &Pubkey) -> Result<&mut Record> {
        self.records
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .ok_or(error!(LedgerError::MissingAccount))
    }
    fn optional(&self, key: &Pubkey) -> Option<&Record> {
        self.records.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}
impl core::Store<Pubkey> for State {
    fn flags(&self, key: &Pubkey) -> std::result::Result<Option<core::Flags<Pubkey>>, core::Error> {
        Ok(self
            .optional(key)
            .filter(|v| v.registered)
            .map(Record::flags))
    }
    fn custody_account(&self, key: &Pubkey) -> std::result::Result<Option<Pubkey>, core::Error> {
        Ok(self
            .optional(key)
            .filter(|v| v.depth > 2 && v.registered)
            .map(|v| v.custodian))
    }
    fn relative(&self, key: &Pubkey) -> std::result::Result<Pubkey, core::Error> {
        self.optional(key)
            .map(|v| v.relative)
            .ok_or(core::Error::InvalidAddress)
    }
}
impl core::AccountingStore<Pubkey> for State {
    fn balances(&self, key: &Pubkey) -> std::result::Result<core::Balances, core::Error> {
        Ok(self
            .optional(key)
            .map(|r| core::Balances {
                debit: r.debit,
                credit: r.credit,
            })
            .unwrap_or_default())
    }
}

pub fn root_address(scope: &Pubkey, id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"ledger", scope.as_ref(), id.as_ref()], &crate::ID)
}
pub fn decode(info: &AccountInfo) -> Result<Record> {
    require_keys_eq!(*info.owner, crate::ID, LedgerError::InvalidAccount);
    let data = info.try_borrow_data()?;
    require!(
        data.len() == SPACE && &data[..8] == MAGIC,
        LedgerError::InvalidAccount
    );
    let r =
        Record::deserialize(&mut &data[8..]).map_err(|_| error!(LedgerError::InvalidAccount))?;
    require!(
        r.kind <= 3 && r.depth >= 2 && r.name.len() <= 64,
        LedgerError::InvalidAccount
    );
    let key = if r.depth == 2 {
        root_address(&r.scope, &r.identifier).0
    } else {
        to_address(&crate::ID, &r.parent, &r.relative).0
    };
    require_keys_eq!(key, *info.key, LedgerError::InvalidAccount);
    Ok(r)
}
fn info<'a, 'info>(
    root: &'a AccountInfo<'info>,
    rest: &'a [AccountInfo<'info>],
    key: &Pubkey,
) -> Result<&'a AccountInfo<'info>> {
    if root.key == key {
        return Ok(root);
    }
    rest.iter()
        .find(|i| i.key == key)
        .ok_or(error!(LedgerError::MissingAccount))
}
fn load(root: &AccountInfo, rest: &[AccountInfo]) -> Result<State> {
    let r = decode(root)?;
    require!(
        r.depth == 2 && r.registered && r.kind == 0 && r.root == *root.key,
        LedgerError::InvalidAccount
    );
    let mut records = vec![(*root.key, r)];
    for a in rest {
        require!(
            !records.iter().any(|(key, _)| key == a.key),
            LedgerError::InvalidAccount
        );
        if a.owner == &crate::ID {
            let r = decode(a)?;
            require_keys_eq!(r.root, *root.key, LedgerError::InvalidAccount);
            require!(r.depth > 2, LedgerError::InvalidAccount);
            records.push((*a.key, r));
        }
    }
    Ok(State { records })
}
fn save(info: &AccountInfo, r: &Record) -> Result<()> {
    require!(info.is_writable, LedgerError::InvalidAccount);
    let mut data = info.try_borrow_mut_data()?;
    data.fill(0);
    data[..8].copy_from_slice(MAGIC);
    r.serialize(&mut &mut data[8..])
        .map_err(|_| error!(LedgerError::InvalidAccount))
}
fn commit<'info>(
    root: &AccountInfo<'info>,
    rest: &[AccountInfo<'info>],
    before: &State,
    after: &State,
) -> Result<()> {
    for (key, r) in &after.records {
        if before.optional(key) != Some(r) {
            save(info(root, rest, key)?, r)?;
            emit!(AccountChanged {
                account: *key,
                root: r.root,
                debit: r.debit,
                credit: r.credit,
                registered: r.registered
            });
        }
    }
    Ok(())
}
fn allocate<'info>(
    payer: &AccountInfo<'info>,
    account: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    seeds: &[&[u8]],
) -> Result<()> {
    require!(
        account.is_writable && account.data_is_empty(),
        LedgerError::InvalidAccount
    );
    require_keys_eq!(
        *account.owner,
        system_program::ID,
        LedgerError::InvalidAccount
    );
    let required = Rent::get()?
        .minimum_balance(SPACE)
        .saturating_sub(account.lamports());
    if required > 0 {
        system_program::transfer(
            CpiContext::new(
                *system.key,
                system_program::Transfer {
                    from: payer.clone(),
                    to: account.clone(),
                },
            ),
            required,
        )?;
    }
    let signer = &[seeds];
    system_program::allocate(
        CpiContext::new_with_signer(
            *system.key,
            system_program::Allocate {
                account_to_allocate: account.clone(),
            },
            signer,
        ),
        SPACE as u64,
    )?;
    system_program::assign(
        CpiContext::new_with_signer(
            *system.key,
            system_program::Assign {
                account_to_assign: account.clone(),
            },
            signer,
        ),
        &crate::ID,
    )
}
fn valid_name(name: &str) -> Result<()> {
    require!(
        !name.is_empty() && name.len() <= 64,
        LedgerError::InvalidName
    );
    Ok(())
}
fn initial(
    root: Pubkey,
    parent: Pubkey,
    relative: Pubkey,
    kind: u8,
    depth: u8,
    bump: u8,
) -> Record {
    Record {
        root,
        parent,
        relative,
        custodian: Pubkey::default(),
        kind,
        token_kind: 0,
        depth,
        registered: true,
        implicit_allowed: true,
        children: 0,
        debit: 0,
        credit: 0,
        name: String::new(),
        scope: Pubkey::default(),
        identifier: Pubkey::default(),
        bump,
    }
}
fn initialize<'info>(
    payer: &AccountInfo<'info>,
    root: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    rest: &[AccountInfo<'info>],
    scope: Pubkey,
    id: Pubkey,
    name: String,
) -> Result<()> {
    valid_name(&name)?;
    let (key, bump) = root_address(&scope, &id);
    require_keys_eq!(key, *root.key, LedgerError::InvalidAccount);
    allocate(
        payer,
        root,
        system,
        &[b"ledger", scope.as_ref(), id.as_ref(), &[bump]],
    )?;
    let mut r = initial(key, Pubkey::default(), id, 0, 2, bump);
    r.scope = scope;
    r.identifier = id;
    r.name = name;
    r.children = 1;
    r.token_kind = if scope == Pubkey::default() { 2 } else { 3 };
    save(root, &r)?;
    let (source, bump) = to_address(&crate::ID, &key, &SOURCE);
    let target = info(root, rest, &source)?;
    allocate(
        payer,
        target,
        system,
        &[b"account", key.as_ref(), SOURCE.as_ref(), &[bump]],
    )?;
    let mut r = initial(key, key, SOURCE, 3, 3, bump);
    r.custodian = source;
    r.name = "Source".into();
    save(target, &r)
}
pub fn add_ledger<'info>(
    ctx: &Context<'info, LedgerAccounts<'info>>,
    id: Pubkey,
    name: String,
) -> Result<()> {
    initialize(
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.root.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        ctx.remaining_accounts,
        ctx.accounts.authority.key(),
        id,
        name,
    )
}
pub fn add_external_token<'info>(
    ctx: Context<'info, RegisterToken<'info>>,
    name: String,
) -> Result<()> {
    initialize(
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.root.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        ctx.remaining_accounts,
        Pubkey::default(),
        ctx.accounts.mint.key(),
        name,
    )
}
fn authority(
    state: &State,
    root: &Pubkey,
    parent: &Pubkey,
    relative: &Pubkey,
    signer: &Pubkey,
) -> Result<()> {
    let r = state.get(root)?;
    if r.scope != Pubkey::default() {
        require_keys_eq!(r.scope, *signer, LedgerError::Unauthorized);
        return Ok(());
    }
    if parent == root {
        require_keys_eq!(*relative, *signer, LedgerError::Unauthorized)
    } else {
        let p = state.get(parent)?;
        let c = state.get(&p.custodian)?;
        require_keys_eq!(c.relative, *signer, LedgerError::Unauthorized);
    }
    Ok(())
}
fn resolve(
    state: &State,
    root: &Pubkey,
    parent: &Pubkey,
    relative: &Pubkey,
) -> Result<core::Endpoint<Pubkey>> {
    let (flags, original, _) =
        core::effective_flags(state, &PdaAddresses(&crate::ID), root, parent, relative)
            .map_err(|_| error!(LedgerError::InvalidAccount))?;
    require!(
        original.is_some() || state.get(parent)?.implicit_allowed,
        LedgerError::InvalidAccount
    );
    Ok(core::Endpoint {
        relative: *relative,
        flags,
    })
}
fn implicit<'info>(
    state: &mut State,
    root: &AccountInfo<'info>,
    rest: &[AccountInfo<'info>],
    payer: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    parent: Pubkey,
    relative: Pubkey,
) -> Result<()> {
    let (key, bump) = to_address(&crate::ID, &parent, &relative);
    if state.optional(&key).is_some() {
        return Ok(());
    }
    let p = state.get(&parent)?;
    require!(p.registered && p.kind < 2, LedgerError::InvalidKind);
    let mut r = initial(
        *root.key,
        parent,
        relative,
        if p.kind == 1 { 3 } else { 2 },
        p.depth
            .checked_add(1)
            .ok_or(error!(LedgerError::InvalidAccount))?,
        bump,
    );
    r.registered = false;
    r.custodian = if parent == *root.key {
        key
    } else {
        p.custodian
    };
    allocate(
        payer,
        info(root, rest, &key)?,
        system,
        &[b"account", parent.as_ref(), relative.as_ref(), &[bump]],
    )?;
    state.records.push((key, r));
    Ok(())
}
pub fn add_account<'info>(
    ctx: &Context<'info, LedgerAccounts<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    name: String,
    credit: bool,
    group: bool,
    implicit_allowed: bool,
) -> Result<()> {
    valid_name(&name)?;
    let root = ctx.accounts.root.to_account_info();
    let before = load(&root, ctx.remaining_accounts)?;
    authority(
        &before,
        root.key,
        &parent,
        &relative,
        &ctx.accounts.authority.key(),
    )?;
    require!(
        relative != SOURCE || parent != *root.key,
        LedgerError::Unauthorized
    );
    let p = before.get(&parent)?;
    require!(p.registered && p.kind < 2, LedgerError::InvalidKind);
    require!(
        !credit || before.get(root.key)?.scope != Pubkey::default(),
        LedgerError::InvalidKind
    );
    let mut state = State {
        records: before.records.clone(),
    };
    implicit(
        &mut state,
        &root,
        ctx.remaining_accounts,
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        parent,
        relative,
    )?;
    let key = to_address(&crate::ID, &parent, &relative).0;
    let r = state.get_mut(&key)?;
    let kind = if group {
        u8::from(credit)
    } else {
        2 + u8::from(credit)
    };
    if r.registered {
        require!(
            r.kind == kind && r.name == name && (!group || r.implicit_allowed == implicit_allowed),
            LedgerError::MetadataConflict
        );
        return Ok(());
    }
    require!(
        !group || (r.debit == 0 && r.credit == 0),
        LedgerError::Nonempty
    );
    require!(
        if credit {
            r.debit <= r.credit
        } else {
            r.credit <= r.debit
        },
        LedgerError::InvalidKind
    );
    r.kind = kind;
    r.name = name;
    r.registered = true;
    r.implicit_allowed = implicit_allowed;
    let p = state.get_mut(&parent)?;
    p.children = p
        .children
        .checked_add(1)
        .ok_or(error!(LedgerError::InvalidAccount))?;
    commit(&root, ctx.remaining_accounts, &before, &state)
}
pub fn remove_account<'info>(
    ctx: &Context<'info, LedgerAccounts<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    group: bool,
) -> Result<()> {
    let root = ctx.accounts.root.to_account_info();
    let before = load(&root, ctx.remaining_accounts)?;
    authority(
        &before,
        root.key,
        &parent,
        &relative,
        &ctx.accounts.authority.key(),
    )?;
    require!(
        relative != SOURCE || parent != *root.key,
        LedgerError::Unauthorized
    );
    let key = to_address(&crate::ID, &parent, &relative).0;
    let Some(r) = before.optional(&key) else {
        return Ok(());
    };
    if !r.registered {
        return Ok(());
    }
    require!((r.kind < 2) == group, LedgerError::InvalidKind);
    require!(
        r.debit == 0 && r.credit == 0 && r.children == 0,
        LedgerError::Nonempty
    );
    let mut state = State {
        records: before.records.clone(),
    };
    let r = state.get_mut(&key)?;
    r.registered = false;
    r.name.clear();
    state.get_mut(&parent)?.children = state
        .get(&parent)?
        .children
        .checked_sub(1)
        .ok_or(error!(LedgerError::InvalidAccount))?;
    commit(&root, ctx.remaining_accounts, &before, &state)
}
fn apply(state: &mut State, changes: Vec<core::BalanceChange<Pubkey>>) -> Result<()> {
    for c in changes {
        let r = state.get_mut(&c.absolute)?;
        require!(
            r.debit == c.before.debit && r.credit == c.before.credit,
            LedgerError::Accounting
        );
        r.debit = c.after.debit;
        r.credit = c.after.credit;
    }
    Ok(())
}
pub fn transfer<'info>(
    ctx: &Context<'info, LedgerAccounts<'info>>,
    from_parent: Pubkey,
    from: Pubkey,
    to_parent: Pubkey,
    to: Pubkey,
    amount: u128,
) -> Result<()> {
    let root = ctx.accounts.root.to_account_info();
    let before = load(&root, ctx.remaining_accounts)?;
    let source = resolve(&before, root.key, &from_parent, &from)?;
    let destination = resolve(&before, root.key, &to_parent, &to)?;
    let internal = before.get(root.key)?.scope != Pubkey::default();
    let changes = if internal {
        authority(
            &before,
            root.key,
            &from_parent,
            &from,
            &ctx.accounts.authority.key(),
        )?;
        core::transfer(
            &before,
            &PdaAddresses(&crate::ID),
            root.key,
            source,
            destination,
            amount,
        )
    } else {
        core::transfer_debits(
            &before,
            &PdaAddresses(&crate::ID),
            root.key,
            source,
            destination,
            &ctx.accounts.authority.key(),
            amount,
        )
    }
    .map_err(|_| error!(LedgerError::Accounting))?;
    let mut state = State {
        records: before.records.clone(),
    };
    for (parent, relative) in [(from_parent, from), (to_parent, to)] {
        implicit(
            &mut state,
            &root,
            ctx.remaining_accounts,
            &ctx.accounts.payer.to_account_info(),
            &ctx.accounts.system_program.to_account_info(),
            parent,
            relative,
        )?;
    }
    apply(&mut state, changes)?;
    commit(&root, ctx.remaining_accounts, &before, &state)
}
pub fn move_tokens<'info>(
    ctx: Context<'info, MoveTokens<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    amount: u64,
    deposit: bool,
) -> Result<()> {
    let root = ctx.accounts.root.to_account_info();
    let before = load(&root, ctx.remaining_accounts)?;
    let r = before.get(root.key)?;
    require!(
        r.scope == Pubkey::default() && r.identifier == ctx.accounts.mint.key(),
        LedgerError::UnsupportedToken
    );
    let leaf = resolve(&before, root.key, &parent, &relative)?;
    require!(
        leaf.flags.account_kind == core::AccountKind::DebitLedger,
        LedgerError::InvalidKind
    );
    core::enforce_is_custodian(&before, root.key, leaf, &ctx.accounts.authority.key())
        .map_err(|_| error!(LedgerError::Unauthorized))?;
    let source = resolve(&before, root.key, root.key, &SOURCE)?;
    if !deposit {
        require!(
            u128::from(ctx.accounts.vault.amount) >= r.credit,
            LedgerError::Undercollateralized
        );
    }
    let (from, to) = if deposit {
        (source, leaf)
    } else {
        (leaf, source)
    };
    let changes = core::transfer(
        &before,
        &PdaAddresses(&crate::ID),
        root.key,
        from,
        to,
        u128::from(amount),
    )
    .map_err(|_| error!(LedgerError::Accounting))?;
    let vault_before = ctx.accounts.vault.amount;
    let wallet_before = ctx.accounts.wallet.amount;
    if deposit {
        require_keys_eq!(
            ctx.accounts.wallet.owner,
            ctx.accounts.funding_authority.key(),
            LedgerError::Unauthorized
        );
    }
    let bump = [r.bump];
    let seeds: &[&[u8]] = &[b"ledger", r.scope.as_ref(), r.identifier.as_ref(), &bump];
    let signer = &[seeds];
    let accounts = TransferChecked {
        from: if deposit {
            ctx.accounts.wallet.to_account_info()
        } else {
            ctx.accounts.vault.to_account_info()
        },
        mint: ctx.accounts.mint.to_account_info(),
        to: if deposit {
            ctx.accounts.vault.to_account_info()
        } else {
            ctx.accounts.wallet.to_account_info()
        },
        authority: if deposit {
            ctx.accounts.funding_authority.to_account_info()
        } else {
            root.clone()
        },
    };
    let cpi = CpiContext::new(ctx.accounts.token_program.key(), accounts);
    token::transfer_checked(
        if deposit {
            cpi
        } else {
            cpi.with_signer(signer)
        },
        amount,
        ctx.accounts.mint.decimals,
    )?;
    ctx.accounts.vault.reload()?;
    ctx.accounts.wallet.reload()?;
    require!(
        if deposit {
            ctx.accounts.vault.amount.checked_sub(vault_before) == Some(amount)
                && wallet_before.checked_sub(ctx.accounts.wallet.amount) == Some(amount)
        } else {
            vault_before.checked_sub(ctx.accounts.vault.amount) == Some(amount)
                && ctx.accounts.wallet.amount.checked_sub(wallet_before) == Some(amount)
        },
        LedgerError::Settlement
    );
    let mut state = State {
        records: before.records.clone(),
    };
    implicit(
        &mut state,
        &root,
        ctx.remaining_accounts,
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        parent,
        relative,
    )?;
    apply(&mut state, changes)?;
    commit(&root, ctx.remaining_accounts, &before, &state)
}

/// Snapshot after a committed mutation. This is a new Solana event schema;
/// it does not claim ERC20 Transfer-event compatibility.
#[event]
pub struct AccountChanged {
    pub account: Pubkey,
    pub root: Pubkey,
    pub debit: u128,
    pub credit: u128,
    pub registered: bool,
}
