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
pub fn name<A: Copy + Eq>(store: &impl ReadStore<A>, absolute: &A) -> Result<String, Error> {
    Ok(store
        .account(absolute)?
        .filter(|a| a.registered)
        .map(|a| String::from(a.name))
        .unwrap_or_default())
}

/// Authenticated asset metadata from the same snapshot as the ledger records.
/// None means the field is undefined; missing input or invalid sources are errors.
/// Hosts supply native values and token metadata without a mutation/settlement host.
pub trait TokenMetadata<A: Copy + Eq>: ReadStore<A> {
    fn token_symbol(&self, ledger: &A) -> Result<Option<String>, Error>;
    fn token_decimals(&self, ledger: &A) -> Result<Option<u8>, Error>;
}

/// Asset metadata belongs to a ledger root, not an application or leaf.
pub fn symbol<A: Copy + Eq>(
    store: &impl TokenMetadata<A>,
    ledger: &A,
) -> Result<Option<String>, Error> {
    root(store, ledger)?;
    store.token_symbol(ledger)
}

/// Base-unit precision, not a UI scaling multiplier. Zero decimals is valid.
pub fn decimals<A: Copy + Eq>(
    store: &impl TokenMetadata<A>,
    ledger: &A,
) -> Result<Option<u8>, Error> {
    root(store, ledger)?;
    store.token_decimals(ledger)
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

/// A complete registered-child index for the same snapshot as account reads.
/// Addresses are absolute; order is host-defined and stable within the snapshot.
pub trait ChildIndex<A: Copy + Eq>: ReadStore<A> {
    fn child_addresses(&self, parent: &A) -> Result<Vec<A>, Error>;
}
/// Authenticated global Root registry from one snapshot. Count is authoritative;
/// entries are unique, append-only and ordered by registration. A host may load
/// only the requested entries; missing in-range input is an error, not absence.
/// No ledger balance records are needed to enumerate this index.
pub trait LedgerIndex<A: Copy + Eq> {
    fn registered_ledger_count(&self) -> Result<u64, Error>;
    fn registered_ledger(&self, index: u64) -> Result<A, Error>;
}
fn unique<A: Copy + Eq>(addresses: &[A]) -> Result<(), Error> {
    for (i, address) in addresses.iter().enumerate() {
        if addresses[..i].contains(address) {
            return Err(Error::InvalidIndex);
        }
    }
    Ok(())
}
fn children<A: Copy + Eq>(
    store: &impl ChildIndex<A>,
    ledger: &A,
    parent_address: &A,
) -> Result<Vec<A>, Error> {
    let parent = parent(store, ledger, parent_address)?;
    let addresses = store.child_addresses(parent_address)?;
    unique(&addresses)?;
    if addresses.len() != parent.children as usize {
        return Err(Error::IncompleteIndex);
    }
    let mut relative = Vec::with_capacity(addresses.len());
    for address in addresses {
        let a = store.account(&address)?.ok_or(Error::InvalidIndex)?;
        if !a.registered
            || a.flags.parent != *parent_address
            || store.to_address(parent_address, &a.relative) != address
        {
            return Err(Error::InvalidIndex);
        }
        relative.push(a.relative);
    }
    Ok(relative)
}
/// Like Solidity subAccounts, return relative identifiers, excluding implicit
/// and removed leaves. A caller can derive each absolute address from its parent.
pub fn sub_accounts<A: Copy + Eq>(
    store: &impl ChildIndex<A>,
    ledger: &A,
    parent: &A,
    start: usize,
    limit: usize,
) -> Result<Vec<A>, Error> {
    Ok(children(store, ledger, parent)?
        .into_iter()
        .skip(start)
        .take(limit)
        .collect())
}
pub fn sub_account<A: Copy + Eq>(
    store: &impl ChildIndex<A>,
    ledger: &A,
    parent: &A,
    index: usize,
) -> Result<A, Error> {
    children(store, ledger, parent)?
        .get(index)
        .copied()
        .ok_or(Error::InvalidIndex)
}
pub fn ledger_count<A: Copy + Eq>(store: &impl LedgerIndex<A>) -> Result<u64, Error> {
    store.registered_ledger_count()
}
pub fn ledger_at<A: Copy + Eq>(store: &impl LedgerIndex<A>, index: u64) -> Result<A, Error> {
    if index >= store.registered_ledger_count()? {
        return Err(Error::InvalidIndex);
    }
    store.registered_ledger(index)
}
pub fn ledgers<A: Copy + Eq>(
    store: &impl LedgerIndex<A>,
    start: u64,
    limit: u64,
) -> Result<Vec<A>, Error> {
    let count = store.registered_ledger_count()?;
    let length = count.saturating_sub(start).min(limit);
    // Grow with authenticated results, never preallocate from an untrusted limit.
    let mut addresses = Vec::new();
    for offset in 0..length {
        addresses.push(store.registered_ledger(start + offset)?);
    }
    Ok(addresses)
}
