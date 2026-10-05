//! Non-mutating LedgerView queries. No mutation module, signer or settlement host.
use crate::ledger_lib::{self as lib, Account, Balances, Error, Flags, ReadStore, StoreView};
use alloc::{string::String, vec::Vec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountView<A> {
    pub absolute: A,
    pub relative: A,
    pub ledger: A,
    pub flags: Flags<A>,
    pub custodian: A,
    pub custodian_is_credit: bool,
    pub registered: bool,
    pub name: String,
    pub balances: Balances,
    /// Parent registration policy only, never enforced as a condition of reading.
    /// This does not imply monetary endpoint eligibility or spending permission.
    pub admitted: bool,
}

fn root<'a, A: Copy + Eq>(
    store: &'a impl ReadStore<A>,
    ledger: &A,
) -> Result<Account<A, &'a str>, Error> {
    let account = store.account(ledger)?.ok_or(Error::InvalidAccount)?;
    if !account.registered || account.flags.depth != 2 || !account.flags.account_kind.is_group() {
        return Err(Error::InvalidAccount);
    }
    Ok(account)
}
fn parent<'a, A: Copy + Eq>(
    store: &'a impl ReadStore<A>,
    ledger: &A,
    parent: &A,
) -> Result<Account<A, &'a str>, Error> {
    root(store, ledger)?;
    let account = store.account(parent)?.ok_or(Error::InvalidAccountGroup)?;
    if !account.registered || !account.flags.account_kind.is_group() {
        return Err(Error::InvalidAccountGroup);
    }
    if lib::ledger(&StoreView(store), parent)? != Some(*ledger) {
        return Err(Error::DifferentRoots);
    }
    Ok(account)
}

/// Resolve registered or implicit account properties without registration,
/// allocation, authority checks or monetary admission checks.
pub fn account<A: Copy + Eq>(
    store: &impl ReadStore<A>,
    ledger: &A,
    parent_address: &A,
    relative: &A,
) -> Result<AccountView<A>, Error> {
    let parent = parent(store, ledger, parent_address)?;
    let view = StoreView(store);
    let (flags, original, absolute) =
        lib::effective_flags(&view, &view, ledger, parent_address, relative)?;
    let (custodian, custodian_is_credit) = lib::custody(&view, ledger, flags, relative)?;
    let stored = store.account(&absolute)?;
    let balances = stored.as_ref().map(|a| a.balances).unwrap_or_default();
    let name = stored
        .filter(|a| a.registered)
        .map(|a| String::from(a.name))
        .unwrap_or_default();
    Ok(AccountView {
        absolute,
        relative: *relative,
        ledger: *ledger,
        flags,
        custodian,
        custodian_is_credit,
        registered: original.is_some(),
        name,
        balances,
        admitted: original.is_some() || parent.implicit_allowed,
    })
}

/// Ledger records a transfer needs writable in this snapshot. Reuse the exact
/// posting walk, including effective flags and common-ancestor cancellation.
/// Changed absent endpoints need storage allocation. Zero/self transfers return
/// no record writes, including when the endpoints have no storage yet.
/// This plans account access, not authorization or custody settlement. The host
/// must revalidate current state and reject any required write not supplied.
pub fn transfer_writable_accounts<A: Copy + Eq>(
    store: &impl ReadStore<A>,
    ledger: &A,
    from: lib::Child<A>,
    to: lib::Child<A>,
    amount: u128,
) -> Result<Vec<A>, Error> {
    let source = account(store, ledger, &from.parent, &from.relative)?;
    let destination = account(store, ledger, &to.parent, &to.relative)?;
    let view = StoreView(store);
    let changes = lib::transfer(
        &view,
        &view,
        ledger,
        lib::Endpoint {
            relative: from.relative,
            flags: source.flags,
        },
        lib::Endpoint {
            relative: to.relative,
            flags: destination.flags,
        },
        amount,
    )?;
    Ok(changes
        .into_iter()
        .filter(|change| change.before != change.after)
        .map(|change| change.absolute)
        .collect())
}

pub fn name<A: Copy + Eq>(store: &impl ReadStore<A>, absolute: &A) -> Result<String, Error> {
    Ok(store
        .account(absolute)?
        .filter(|a| a.registered)
        .map(|a| String::from(a.name))
        .unwrap_or_default())
}

/// Metadata stored at registration, matching the original LedgerLib snapshot.
/// The optional return shape is retained for read-client compatibility; a valid
/// initialized ledger always has a symbol and decimals, including internal units.
pub fn symbol<A: Copy + Eq>(
    store: &impl ReadStore<A>,
    ledger: &A,
) -> Result<Option<String>, Error> {
    Ok(Some(String::from(root(store, ledger)?.symbol)))
}
/// Base-unit precision captured at registration; zero decimals is valid.
pub fn decimals<A: Copy + Eq>(store: &impl ReadStore<A>, ledger: &A) -> Result<Option<u8>, Error> {
    Ok(Some(root(store, ledger)?.decimals))
}
pub fn debit_balance_of<A: Copy + Eq>(
    store: &impl ReadStore<A>,
    ledger: &A,
    parent_address: &A,
    relative: &A,
) -> Result<u128, Error> {
    parent(store, ledger, parent_address)?;
    Ok(store
        .account(&store.to_address(parent_address, relative))?
        .map(|a| a.balances.debit)
        .unwrap_or_default())
}
pub fn credit_balance_of<A: Copy + Eq>(
    store: &impl ReadStore<A>,
    ledger: &A,
    parent_address: &A,
    relative: &A,
) -> Result<u128, Error> {
    parent(store, ledger, parent_address)?;
    Ok(store
        .account(&store.to_address(parent_address, relative))?
        .map(|a| a.balances.credit)
        .unwrap_or_default())
}
/// Preserve Solidity's unsigned, polarity-dependent net balance. A negative net
/// is an error; both gross balances remain separately inspectable.
pub fn balance_of<A: Copy + Eq>(
    store: &impl ReadStore<A>,
    ledger: &A,
    parent_address: &A,
    relative: &A,
) -> Result<u128, Error> {
    parent(store, ledger, parent_address)?;
    let view = StoreView(store);
    let (flags, _, absolute) =
        lib::effective_flags(&view, &view, ledger, parent_address, relative)?;
    lib::custody(&view, ledger, flags, relative)?;
    let balances = store
        .account(&absolute)?
        .map(|a| a.balances)
        .unwrap_or_default();
    let (positive, negative) = if flags.account_kind.is_credit() {
        (balances.credit, balances.debit)
    } else {
        (balances.debit, balances.credit)
    };
    positive
        .checked_sub(negative)
        .ok_or(Error::InsufficientBalance)
}
/// The original LedgerView returns the root's gross debit balance, not its net.
pub fn total_supply<A: Copy + Eq>(store: &impl ReadStore<A>, ledger: &A) -> Result<u128, Error> {
    Ok(root(store, ledger)?.balances.debit)
}
pub fn ledger<A: Copy + Eq>(store: &impl ReadStore<A>, absolute: &A) -> Result<Option<A>, Error> {
    lib::ledger(&StoreView(store), absolute)
}

/// Authenticated, maintained child slots from the same snapshot as account reads.
/// Positions are zero-based, in insertion order with swap-and-pop removal.
/// Values are relative identities, except Root's children are ledger addresses.
/// Hosts must authenticate each slot's parent and position. An unavailable slot
/// is an error, never permission to reconstruct the index from account records.
pub trait ChildIndex<A: Copy + Eq>: ReadStore<A> {
    fn child_at(&self, parent: &A, index: u32) -> Result<A, Error>;
}

/// Original one-based subAccountIndex; unregistered accounts return zero.
pub fn sub_account_index<A: Copy + Eq>(
    store: &impl ReadStore<A>,
    absolute: &A,
) -> Result<u32, Error> {
    Ok(store
        .account(absolute)?
        .filter(|a| a.registered)
        .map_or(0, |a| a.sub_index))
}
fn enumeration_parent<'a, A: Copy + Eq>(
    store: &'a impl ReadStore<A>,
    ledger: &A,
    parent_address: &A,
) -> Result<Account<A, &'a str>, Error> {
    if ledger == parent_address {
        let account = store
            .account(parent_address)?
            .ok_or(Error::InvalidAccountGroup)?;
        if account.flags.depth == 1 {
            if !account.registered || account.flags.account_kind != lib::AccountKind::DebitGroup {
                return Err(Error::InvalidAccountGroup);
            }
            return Ok(account);
        }
    }
    parent(store, ledger, parent_address)
}
/// Stored count of registered immediate children; no child records are required.
pub fn sub_account_count<A: Copy + Eq>(
    store: &impl ReadStore<A>,
    ledger: &A,
    parent: &A,
) -> Result<u32, Error> {
    Ok(enumeration_parent(store, ledger, parent)?.children)
}
/// Read only the requested maintained child slots. No sibling records or sorting.
/// Root returns ledger addresses; other groups return relative identities.
pub fn sub_accounts<A: Copy + Eq>(
    store: &impl ChildIndex<A>,
    ledger: &A,
    parent: &A,
    start: usize,
    limit: usize,
) -> Result<Vec<A>, Error> {
    let count = enumeration_parent(store, ledger, parent)?.children as usize;
    let end = start.saturating_add(limit).min(count);
    let mut result = Vec::with_capacity(end.saturating_sub(start));
    for index in start..end {
        result.push(store.child_at(parent, index as u32)?);
    }
    Ok(result)
}
pub fn sub_account<A: Copy + Eq>(
    store: &impl ChildIndex<A>,
    ledger: &A,
    parent: &A,
    index: usize,
) -> Result<A, Error> {
    let count = enumeration_parent(store, ledger, parent)?.children;
    if index >= count as usize {
        return Err(Error::InvalidIndex);
    }
    store.child_at(parent, index as u32)
}
/// Ledger discovery is Root's ordinary child enumeration.
pub fn ledger_count<A: Copy + Eq>(store: &impl ReadStore<A>, root: &A) -> Result<u32, Error> {
    sub_account_count(store, root, root)
}
pub fn ledger_at<A: Copy + Eq>(
    store: &impl ChildIndex<A>,
    root: &A,
    index: usize,
) -> Result<A, Error> {
    sub_account(store, root, root, index)
}
pub fn ledgers<A: Copy + Eq>(
    store: &impl ChildIndex<A>,
    root: &A,
    start: usize,
    limit: usize,
) -> Result<Vec<A>, Error> {
    sub_accounts(store, root, root, start, limit)
}
