//! Standalone Ledger operations over authenticated, transactional host services.
//! Mirrors Ledger.sol; the original posting walk remains in ledger_lib.
use crate::ledger_lib::{self as lib, AccountKind, Balances, Error, Flags, TokenKind};
use alloc::{string::String, vec::Vec};

pub use lib::{Account, Child, Root};
use lib::{ReadStore, StoreView as View};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Authority,
    TokenPayer,
}

/// Both balances are independently observed custody state in native base units.
/// The host binds the vault to this root and the wallet to this operation.
#[derive(Clone, Copy, Debug)]
pub struct TokenBalances<A> {
    pub asset: A,
    pub owner: A,
    pub vault: u128,
    pub wallet: u128,
}

/// Trusted host implementation, not an interface supplied by an end user.
///
/// Authentication must bind identities to the current call/transaction. Authority
/// is the acting application or direct holder, not necessarily the fee payer or
/// transaction signer. A raw address argument is never authentication. The host
/// performs cryptographic verification or consumes its runtime's verified context.
/// The command is supplied for hosts that verify an operation-specific proof;
/// bind it together with the host's root, asset, wallet and service/chain context.
///
/// Account reads validate ownership, identity and root membership. `put` stages
/// logical state; allocating storage must not grant authority or registration.
/// `atomic` covers authentication, staged writes, token movement and `commit`.
/// On any error all effects except transaction fees must roll back. A runtime
/// with transaction-wide rollback must propagate errors out of its entry point;
/// it must not catch an error and commit the surrounding transaction.
pub trait Host<A: Copy + Eq>: ReadStore<A> + Sized {
    type Error: From<Error>;
    fn root(&self) -> Root<A>;
    fn authenticate(&self, role: Role, command: &Command<A>) -> Result<A, Self::Error>;
    fn put(&mut self, address: A, account: Account<A>) -> Result<(), Self::Error>;
    /// Update existing fields without reading/copying unrelated metadata.
    fn set_balances(&mut self, address: A, balances: Balances) -> Result<(), Self::Error>;
    fn set_children(&mut self, address: A, children: u32) -> Result<(), Self::Error>;
    fn token_balances(&mut self) -> Result<TokenBalances<A>, Self::Error>;
    fn move_tokens(&mut self, deposit: bool, amount: u128) -> Result<(), Self::Error>;
    fn commit(&mut self) -> Result<(), Self::Error>;
    fn atomic(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<(), Self::Error>,
    ) -> Result<(), Self::Error>;
}

pub enum Command<A> {
    Initialize {
        name: String,
    },
    Add {
        child: Child<A>,
        name: String,
        kind: AccountKind,
        implicit_allowed: bool,
    },
    Remove {
        child: Child<A>,
        group: bool,
    },
    Transfer {
        from: Child<A>,
        to: Child<A>,
        amount: u128,
    },
    MoveTokens {
        child: Child<A>,
        amount: u128,
        deposit: bool,
    },
}

/// Public service boundary. Hosts provide mechanisms; Ledger owns permission,
/// lifecycle, admission, posting and exact-settlement policy.
pub fn execute<A: Copy + Eq, H: Host<A>>(
    host: &mut H,
    command: Command<A>,
) -> Result<(), H::Error> {
    host.atomic(|host| {
        let authority = host.authenticate(Role::Authority, &command)?;
        match command {
            Command::Initialize { name } => initialize(host, authority, name)?,
            Command::Add {
                child,
                name,
                kind,
                implicit_allowed,
            } => add_account(host, authority, child, name, kind, implicit_allowed)?,
            Command::Remove { child, group } => remove_account(host, authority, child, group)?,
            Command::Transfer { from, to, amount } => transfer(host, authority, from, to, amount)?,
            Command::MoveTokens {
                child,
                amount,
                deposit,
            } => move_tokens(host, authority, child, amount, deposit)?,
        }
        host.commit()
    })
}

fn get<'a, A: Copy + Eq>(host: &'a impl Host<A>, key: &A) -> Result<Account<A, &'a str>, Error> {
    host.account(key)?.ok_or(Error::MissingAccount)
}
fn valid_name(name: &str) -> Result<(), Error> {
    if name.is_empty() || name.len() > 64 {
        return Err(Error::InvalidName);
    }
    Ok(())
}
fn authorize<A: Copy + Eq>(
    host: &impl Host<A>,
    authority: A,
    child: Child<A>,
) -> Result<(), Error> {
    let root = host.root();
    let expected = if let Some(owner) = root.authority {
        owner
    } else if child.parent == root.address {
        child.relative
    } else {
        get(host, &get(host, &child.parent)?.custodian)?.relative
    };
    if authority != expected {
        return Err(Error::Unauthorized);
    }
    Ok(())
}
fn ordinary<A: Copy + Eq>(root: Root<A>, child: Child<A>) -> Result<(), Error> {
    if child.parent == root.address && child.relative == root.source {
        return Err(Error::Unauthorized);
    }
    Ok(())
}
fn resolve<A: Copy + Eq>(host: &impl Host<A>, child: Child<A>) -> Result<lib::Endpoint<A>, Error> {
    let view = View(host);
    let (flags, original, _) = lib::effective_flags(
        &view,
        &view,
        &host.root().address,
        &child.parent,
        &child.relative,
    )
    .map_err(|_| Error::InvalidAccount)?;
    if original.is_none() && !get(host, &child.parent)?.implicit_allowed {
        return Err(Error::InvalidAccount);
    }
    Ok(lib::Endpoint {
        relative: child.relative,
        flags,
    })
}
fn implicit<A: Copy + Eq, H: Host<A>>(host: &mut H, child: Child<A>) -> Result<A, H::Error> {
    let key = host.to_address(&child.parent, &child.relative);
    if host.account(&key)?.is_none() {
        let parent = get(host, &child.parent)?;
        if !parent.registered || !parent.flags.account_kind.is_group() {
            return Err(Error::InvalidKind.into());
        }
        let account = Account {
            flags: Flags {
                parent: child.parent,
                account_kind: if parent.flags.account_kind.is_credit() {
                    AccountKind::CreditLedger
                } else {
                    AccountKind::DebitLedger
                },
                token_kind: TokenKind::Unregistered,
                depth: parent
                    .flags
                    .depth
                    .checked_add(1)
                    .ok_or(Error::InvalidAccount)?,
            },
            relative: child.relative,
            custodian: if child.parent == host.root().address {
                key
            } else {
                parent.custodian
            },
            registered: false,
            implicit_allowed: true,
            children: 0,
            balances: Balances::default(),
            name: String::new(),
        };
        host.put(key, account)?;
    }
    Ok(key)
}
fn initialize<A: Copy + Eq, H: Host<A>>(
    host: &mut H,
    authority: A,
    name: String,
) -> Result<(), H::Error> {
    valid_name(&name)?;
    let root = host.root();
    if root.authority.is_some_and(|owner| owner != authority) {
        return Err(Error::Unauthorized.into());
    }
    if host.account(&root.address)?.is_some() {
        return Err(Error::InvalidAccount.into());
    }
    let account = Account {
        flags: Flags {
            parent: root.parent,
            account_kind: AccountKind::DebitGroup,
            token_kind: if root.authority.is_some() {
                TokenKind::Internal
            } else {
                TokenKind::External
            },
            depth: 2,
        },
        relative: root.identifier,
        custodian: root.parent,
        registered: true,
        implicit_allowed: true,
        children: 1,
        balances: Balances::default(),
        name,
    };
    host.put(root.address, account)?;
    let key = host.to_address(&root.address, &root.source);
    host.put(
        key,
        Account {
            flags: Flags {
                parent: root.address,
                account_kind: AccountKind::CreditLedger,
                token_kind: TokenKind::Unregistered,
                depth: 3,
            },
            relative: root.source,
            custodian: key,
            registered: true,
            implicit_allowed: true,
            children: 0,
            balances: Balances::default(),
            name: "Source".into(),
        },
    )
}
fn add_account<A: Copy + Eq, H: Host<A>>(
    host: &mut H,
    authority: A,
    child: Child<A>,
    name: String,
    kind: AccountKind,
    implicit_allowed: bool,
) -> Result<(), H::Error> {
    valid_name(&name)?;
    authorize(host, authority, child)?;
    ordinary(host.root(), child)?;
    let parent = get(host, &child.parent)?;
    if !parent.registered
        || !parent.flags.account_kind.is_group()
        || (kind.is_credit() && host.root().authority.is_none())
    {
        return Err(Error::InvalidKind.into());
    }
    let children = parent.children;
    let key = implicit(host, child)?;
    let mut account = get(host, &key)?;
    if account.registered {
        if account.flags.account_kind != kind
            || account.name != name
            || (kind.is_group() && account.implicit_allowed != implicit_allowed)
        {
            return Err(Error::MetadataConflict.into());
        }
        return Ok(());
    }
    if kind.is_group() && account.balances != Balances::default() {
        return Err(Error::NonemptyAccount.into());
    }
    if (kind.is_credit() && account.balances.debit > account.balances.credit)
        || (!kind.is_credit() && account.balances.credit > account.balances.debit)
    {
        return Err(Error::InvalidKind.into());
    }
    account.flags.account_kind = kind;
    account.registered = true;
    account.implicit_allowed = implicit_allowed;
    let children = children.checked_add(1).ok_or(Error::InvalidAccount)?;
    let account = account.with_name(name);
    host.put(key, account)?;
    host.set_children(child.parent, children)
}
fn remove_account<A: Copy + Eq, H: Host<A>>(
    host: &mut H,
    authority: A,
    child: Child<A>,
    group: bool,
) -> Result<(), H::Error> {
    authorize(host, authority, child)?;
    ordinary(host.root(), child)?;
    // Preserve parent validation before no-op removal, without monetary admission.
    let view = View(&*host);
    lib::effective_flags(
        &view,
        &view,
        &host.root().address,
        &child.parent,
        &child.relative,
    )
    .map_err(|_| Error::InvalidAccount)?;
    let key = host.to_address(&child.parent, &child.relative);
    let Some(mut account) = host.account(&key)? else {
        return Ok(());
    };
    if !account.registered {
        return Ok(());
    }
    if account.flags.account_kind.is_group() != group {
        return Err(Error::InvalidKind.into());
    }
    if account.balances != Balances::default() || account.children != 0 {
        return Err(Error::NonemptyAccount.into());
    }
    account.registered = false;
    let account = account.with_name(String::new());
    let children = get(host, &child.parent)?
        .children
        .checked_sub(1)
        .ok_or(Error::InvalidAccount)?;
    host.put(key, account)?;
    host.set_children(child.parent, children)
}
fn apply<A: Copy + Eq, H: Host<A>>(
    host: &mut H,
    changes: Vec<lib::BalanceChange<A>>,
) -> Result<(), H::Error> {
    for change in changes {
        let account = get(host, &change.absolute)?;
        if account.balances != change.before {
            return Err(Error::Accounting.into());
        }
        host.set_balances(change.absolute, change.after)?;
    }
    Ok(())
}
fn transfer<A: Copy + Eq, H: Host<A>>(
    host: &mut H,
    authority: A,
    from: Child<A>,
    to: Child<A>,
    amount: u128,
) -> Result<(), H::Error> {
    let source = resolve(host, from)?;
    let destination = resolve(host, to)?;
    let view = View(&*host);
    let root = host.root();
    let changes = if root.authority.is_some() {
        authorize(host, authority, from)?;
        lib::transfer(&view, &view, &root.address, source, destination, amount)
    } else {
        lib::transfer_debits(
            &view,
            &view,
            &root.address,
            source,
            destination,
            &authority,
            amount,
        )
    }
    .map_err(|_| Error::Accounting)?;
    implicit(host, from)?;
    implicit(host, to)?;
    apply(host, changes)
}
fn move_tokens<A: Copy + Eq, H: Host<A>>(
    host: &mut H,
    authority: A,
    child: Child<A>,
    amount: u128,
    deposit: bool,
) -> Result<(), H::Error> {
    let root = host.root();
    let before = host.token_balances()?;
    if root.authority.is_some() || root.identifier != before.asset {
        return Err(Error::UnsupportedToken.into());
    }
    let leaf = resolve(host, child)?;
    if leaf.flags.account_kind != AccountKind::DebitLedger {
        return Err(Error::InvalidKind.into());
    }
    let view = View(&*host);
    lib::enforce_is_custodian(&view, &root.address, leaf, &authority)
        .map_err(|_| Error::Unauthorized)?;
    let source = resolve(
        host,
        Child {
            parent: root.address,
            relative: root.source,
        },
    )?;
    if !deposit && before.vault < get(host, &root.address)?.balances.credit {
        return Err(Error::Undercollateralized.into());
    }
    let (from, to) = if deposit {
        (source, leaf)
    } else {
        (leaf, source)
    };
    let changes = lib::transfer(&view, &view, &root.address, from, to, amount)
        .map_err(|_| Error::Accounting)?;
    if deposit
        && host.authenticate(
            Role::TokenPayer,
            &Command::MoveTokens {
                child,
                amount,
                deposit,
            },
        )? != before.owner
    {
        return Err(Error::Unauthorized.into());
    }
    host.move_tokens(deposit, amount)?;
    let after = host.token_balances()?;
    let exact = if deposit {
        after.vault.checked_sub(before.vault) == Some(amount)
            && before.wallet.checked_sub(after.wallet) == Some(amount)
    } else {
        before.vault.checked_sub(after.vault) == Some(amount)
            && after.wallet.checked_sub(before.wallet) == Some(amount)
    };
    if !exact || after.asset != before.asset || after.owner != before.owner {
        return Err(Error::Settlement.into());
    }
    implicit(host, child)?;
    apply(host, changes)
}
