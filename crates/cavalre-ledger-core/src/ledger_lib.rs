//! Corresponds to cavalre-contracts/modules/ledger/LedgerLib.sol.
//! Shared accounting rules over host-authenticated state. No platform SDK,
//! serialization format, account allocation or signature verification lives here.

use alloc::string::String;

/// Logical account state, independent of serialization and physical allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account<A, Name = String> {
    pub flags: Flags<A>,
    pub relative: A,
    pub custodian: A,
    pub registered: bool,
    pub implicit_allowed: bool,
    pub children: u32,
    /// One-based position in the parent's registered children; zero when unregistered.
    pub sub_index: u32,
    pub balances: Balances,
    pub name: Name,
}

impl<A: Copy, Name: AsRef<str>> Account<A, Name> {
    /// Borrow metadata; flags/balance reads must not allocate or clone a name.
    pub fn as_ref(&self) -> Account<A, &str> {
        Account {
            flags: self.flags,
            relative: self.relative,
            custodian: self.custodian,
            registered: self.registered,
            implicit_allowed: self.implicit_allowed,
            children: self.children,
            sub_index: self.sub_index,
            balances: self.balances,
            name: self.name.as_ref(),
        }
    }
}

impl<A, Name> Account<A, Name> {
    /// Supply an owned name only when creating or changing account metadata.
    pub fn with_name(self, name: String) -> Account<A> {
        Account {
            flags: self.flags,
            relative: self.relative,
            custodian: self.custodian,
            registered: self.registered,
            implicit_allowed: self.implicit_allowed,
            children: self.children,
            sub_index: self.sub_index,
            balances: self.balances,
            name,
        }
    }
}

/// Host-authenticated root identity. `authority: None` means external custody;
/// an accounting-only root has an explicit owner and no withdrawal entitlement.
#[derive(Clone, Copy, Debug)]
pub struct Root<A> {
    pub address: A,
    pub parent: A,
    pub identifier: A,
    pub source: A,
    pub authority: Option<A>,
}

#[derive(Clone, Copy, Debug)]
pub struct Child<A> {
    pub parent: A,
    pub relative: A,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountKind {
    DebitGroup,
    CreditGroup,
    DebitLedger,
    CreditLedger,
}

impl AccountKind {
    pub fn is_group(self) -> bool {
        matches!(self, Self::DebitGroup | Self::CreditGroup)
    }

    pub fn is_credit(self) -> bool {
        matches!(self, Self::CreditGroup | Self::CreditLedger)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Unregistered,
    Native,
    External,
    Internal,
}

/// Decoded Solidity flags with a host-defined address type.
/// None in Store::flags represents unregistered metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags<A> {
    pub parent: A,
    pub account_kind: AccountKind,
    pub token_kind: TokenKind,
    pub depth: u8,
}

/// Read boundary corresponding to the original flags, custody and subaccount
/// mappings. Missing metadata does NOT imply a zero balance or missing storage.
/// Implementations must authenticate ownership, account identity and decoding;
/// malformed stored data must return an error, not None.
pub trait Store<A> {
    fn flags(&self, absolute: &A) -> Result<Option<Flags<A>>, Error>;
    fn custody_account(&self, absolute: &A) -> Result<Option<A>, Error>;
    fn relative(&self, absolute: &A) -> Result<A, Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidAccountGroup,
    DifferentRoots,
    InvalidAddress,
    DepthOverflow,
    InvalidLedgerAccount,
    InsufficientBalance,
    Overflow,
    Unauthorized,
    NonemptyAccount,
    RegistrationRequired,
    InvalidAccount,
    InvalidKind,
    InvalidName,
    MetadataConflict,
    MissingAccount,
    Accounting,
    UnsupportedToken,
    Settlement,
    Undercollateralized,
    IncompleteIndex,
    InvalidIndex,
    InvalidMetadata,
    UnsupportedMetadata,
}

/// Host-specific deterministic address derivation. The implementation must bind
/// both parent and relative identity in its own service domain. It does not
/// allocate storage or register a leaf. Never accept an address claimed by an
/// untrusted caller in place of deriving it.
pub trait AddressDerivation<A> {
    fn to_address(&self, parent: &A, relative: &A) -> A;
}

/// Roots identify themselves; registered descendants resolve through custody.
/// An implicit leaf requires its parent context, exactly as in LedgerLib.
pub fn ledger<A: Copy + Eq>(store: &impl Store<A>, absolute: &A) -> Result<Option<A>, Error> {
    if let Some(custodian) = store.custody_account(absolute)? {
        let flags = store.flags(&custodian)?.ok_or(Error::InvalidAddress)?;
        return Ok(Some(flags.parent));
    }
    Ok(store
        .flags(absolute)?
        .and_then(|flags| (flags.depth == 2 && flags.account_kind.is_group()).then_some(*absolute)))
}

/// Effective flags, original registered flags, and absolute address.
pub type EffectiveFlags<A> = (Flags<A>, Option<Flags<A>>, A);

/// Returns (effective flags, original registered flags, absolute address).
/// Unregistered leaves inherit their parent's polarity and next depth without
/// registering the leaf, changing custody, or requiring the receiver to sign.
pub fn effective_flags<A: Copy + Eq>(
    store: &impl Store<A>,
    addresses: &impl AddressDerivation<A>,
    ledger_address: &A,
    parent: &A,
    relative: &A,
) -> Result<EffectiveFlags<A>, Error> {
    let parent_flags = store.flags(parent)?.ok_or(Error::InvalidAccountGroup)?;
    if !parent_flags.account_kind.is_group() {
        return Err(Error::InvalidAccountGroup);
    }
    if ledger(store, parent)? != Some(*ledger_address) {
        return Err(Error::DifferentRoots);
    }
    let absolute = addresses.to_address(parent, relative);
    if let Some(original) = store.flags(&absolute)? {
        return Ok((original, Some(original), absolute));
    }
    let effective = Flags {
        parent: *parent,
        account_kind: if parent_flags.account_kind.is_credit() {
            AccountKind::CreditLedger
        } else {
            AccountKind::DebitLedger
        },
        token_kind: TokenKind::Unregistered,
        depth: parent_flags
            .depth
            .checked_add(1)
            .ok_or(Error::DepthOverflow)?,
    };
    Ok((effective, None, absolute))
}

/// Return the direct child's relative identity and polarity. This identifies
/// custody; it does not by itself authorize a transaction.
pub fn custody<A: Copy + Eq>(
    store: &impl Store<A>,
    ledger_address: &A,
    effective: Flags<A>,
    relative: &A,
) -> Result<(A, bool), Error> {
    if effective.parent == *ledger_address {
        return Ok((*relative, effective.account_kind.is_credit()));
    }
    let custodian = store
        .custody_account(&effective.parent)?
        .ok_or(Error::InvalidAddress)?;
    let flags = store.flags(&custodian)?.ok_or(Error::InvalidAddress)?;
    if flags.parent != *ledger_address {
        return Err(Error::DifferentRoots);
    }
    Ok((store.relative(&custodian)?, flags.account_kind.is_credit()))
}

/// Current gross balances, not cumulative transaction totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Balances {
    pub debit: u128,
    pub credit: u128,
}

/// Gross balance column changed by a posting, regardless of group polarity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalanceSide {
    Debit,
    Credit,
}
impl Balances {
    pub fn get(&self, side: BalanceSide) -> u128 {
        match side {
            BalanceSide::Debit => self.debit,
            BalanceSide::Credit => self.credit,
        }
    }
}

pub trait AccountingStore<A>: Store<A> {
    fn balances(&self, absolute: &A) -> Result<Balances, Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint<A> {
    pub relative: A,
    pub flags: Flags<A>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BalanceChange<A> {
    pub absolute: A,
    pub before: Balances,
    pub after: Balances,
    /// Credit/debit *posting* events, including zero-amount postings. The value
    /// selects the resulting gross column; event direction is not polarity.
    pub credit: Option<BalanceSide>,
    pub debit: Option<BalanceSide>,
}

/// Original depth-aligned transfer walk. Callers resolve effective flags and
/// authorize the operation first. The returned writes must be committed
/// atomically against the same authenticated state, including native settlement.
/// Staging writes avoids partial host mutations on a late arithmetic failure.
/// This internal posting preserves the original same-account no-op behavior;
/// public debit transfers must check funds before invoking it.
pub fn transfer<A: Copy + Eq>(
    store: &impl AccountingStore<A>,
    addresses: &impl AddressDerivation<A>,
    ledger_address: &A,
    from: Endpoint<A>,
    to: Endpoint<A>,
    amount: u128,
) -> Result<alloc::vec::Vec<BalanceChange<A>>, Error> {
    let mut changes = alloc::vec::Vec::new();
    if from.flags.account_kind.is_group() || to.flags.account_kind.is_group() {
        return Err(Error::InvalidLedgerAccount);
    }
    let mut from_address = addresses.to_address(&from.flags.parent, &from.relative);
    let mut to_address = addresses.to_address(&to.flags.parent, &to.relative);
    if from_address == to_address {
        return Ok(changes);
    }
    let from_credit = from.flags.account_kind.is_credit();
    let to_credit = to.flags.account_kind.is_credit();
    let mut from_flags = from.flags;
    let mut to_flags = to.flags;
    let mut depth = from_flags.depth.max(to_flags.depth);
    if from_flags.depth < 3 || to_flags.depth < 3 {
        return Err(Error::InvalidLedgerAccount);
    }
    // At most both paths up to the root, with the common root counted once.
    // Reserve once: a bump allocator cannot recycle Vec growth allocations.
    changes.reserve_exact(usize::from(from.flags.depth) + usize::from(to.flags.depth) - 3);
    loop {
        if from.flags.depth >= depth {
            update(
                store,
                &mut changes,
                from_address,
                from_credit,
                from_credit,
                amount,
            )?;
            if depth > 2 {
                (from_address, from_flags) = ancestor(store, ledger_address, from_flags)?;
            }
        }
        if to.flags.depth >= depth {
            update(
                store,
                &mut changes,
                to_address,
                to_credit,
                !to_credit,
                amount,
            )?;
            if depth > 2 {
                (to_address, to_flags) = ancestor(store, ledger_address, to_flags)?;
            }
        }
        if depth == 2 || (from_address == to_address && from_credit == to_credit) {
            return Ok(changes);
        }
        depth -= 1;
    }
}

fn ancestor<A: Copy + Eq>(
    store: &impl Store<A>,
    ledger_address: &A,
    child: Flags<A>,
) -> Result<(A, Flags<A>), Error> {
    let parent = store
        .flags(&child.parent)?
        .ok_or(Error::InvalidAccountGroup)?;
    if !parent.account_kind.is_group() || parent.depth.checked_add(1) != Some(child.depth) {
        return Err(Error::InvalidAccountGroup);
    }
    if parent.depth < 2 || (parent.depth == 2 && child.parent != *ledger_address) {
        return Err(Error::DifferentRoots);
    }
    Ok((child.parent, parent))
}

fn update<A: Copy + Eq>(
    store: &impl AccountingStore<A>,
    changes: &mut alloc::vec::Vec<BalanceChange<A>>,
    absolute: A,
    credit: bool,
    increase: bool,
    amount: u128,
) -> Result<(), Error> {
    let index = if let Some(index) = changes.iter().position(|c| c.absolute == absolute) {
        index
    } else {
        let before = store.balances(&absolute)?;
        changes.push(BalanceChange {
            absolute,
            before,
            after: before,
            credit: None,
            debit: None,
        });
        changes.len() - 1
    };
    let change = &mut changes[index];
    let side = if credit {
        BalanceSide::Credit
    } else {
        BalanceSide::Debit
    };
    if credit == increase {
        change.credit = Some(side);
    } else {
        change.debit = Some(side);
    }
    let balances = &mut change.after;
    let balance = if credit {
        &mut balances.credit
    } else {
        &mut balances.debit
    };
    *balance = if increase {
        balance.checked_add(amount).ok_or(Error::Overflow)?
    } else {
        balance
            .checked_sub(amount)
            .ok_or(Error::InsufficientBalance)?
    };
    Ok(())
}

/// Custodian identity must already be authenticated by the host. This is not
/// signature verification and cannot turn an instruction argument into authority.
pub fn enforce_is_custodian<A: Copy + Eq>(
    store: &impl Store<A>,
    ledger_address: &A,
    account: Endpoint<A>,
    authority: &A,
) -> Result<(), Error> {
    if custody(store, ledger_address, account.flags, &account.relative)?.0 != *authority {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

/// Public same-custodian debit transfer, including checks before self no-ops.
pub fn transfer_debits<A: Copy + Eq>(
    store: &impl AccountingStore<A>,
    addresses: &impl AddressDerivation<A>,
    ledger_address: &A,
    from: Endpoint<A>,
    to: Endpoint<A>,
    authority: &A,
    amount: u128,
) -> Result<alloc::vec::Vec<BalanceChange<A>>, Error> {
    if from.flags.account_kind != AccountKind::DebitLedger
        || to.flags.account_kind != AccountKind::DebitLedger
    {
        return Err(Error::InvalidLedgerAccount);
    }
    enforce_is_custodian(store, ledger_address, from, authority)?;
    enforce_is_custodian(store, ledger_address, to, authority)?;
    let absolute = addresses.to_address(&from.flags.parent, &from.relative);
    if store.balances(&absolute)?.debit < amount {
        return Err(Error::InsufficientBalance);
    }
    transfer(store, addresses, ledger_address, from, to, amount)
}

/// Distinguished parent of all token and accounting ledgers.
pub const ROOT_NAME: &str = "Root";

/// State reads shared by queries and mutations. None means confirmed absence;
/// omitted or unavailable state must return an error. Implementations validate
/// owner, identity and ledger membership and read a consistent state snapshot.
/// Names borrow host storage, so walking flags/custody/balances does not allocate.
pub trait ReadStore<A: Copy + Eq>: AddressDerivation<A> {
    fn account(&self, address: &A) -> Result<Option<Account<A, &str>>, Error>;
}

/// Read-only adaptation to the original LedgerLib interfaces.
pub struct StoreView<'a, S>(pub &'a S);
impl<A: Copy + Eq, S: ReadStore<A>> Store<A> for StoreView<'_, S> {
    fn flags(&self, key: &A) -> Result<Option<Flags<A>>, Error> {
        Ok(self
            .0
            .account(key)?
            .filter(|a| a.registered)
            .map(|a| a.flags))
    }
    fn custody_account(&self, key: &A) -> Result<Option<A>, Error> {
        Ok(self
            .0
            .account(key)?
            .filter(|a| a.registered && a.flags.depth > 2)
            .map(|a| a.custodian))
    }
    fn relative(&self, key: &A) -> Result<A, Error> {
        Ok(self.0.account(key)?.ok_or(Error::MissingAccount)?.relative)
    }
}
impl<A: Copy + Eq, S: ReadStore<A>> AccountingStore<A> for StoreView<'_, S> {
    fn balances(&self, key: &A) -> Result<Balances, Error> {
        Ok(self.0.account(key)?.map(|a| a.balances).unwrap_or_default())
    }
}
impl<A: Copy + Eq, S: ReadStore<A>> AddressDerivation<A> for StoreView<'_, S> {
    fn to_address(&self, parent: &A, relative: &A) -> A {
        self.0.to_address(parent, relative)
    }
}
