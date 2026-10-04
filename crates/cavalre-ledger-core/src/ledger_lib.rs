//! Corresponds to cavalre-contracts/modules/ledger/LedgerLib.sol.
//! Shared, read-only rules over host-authenticated state. No platform SDK,
//! serialization format, account allocation or signature verification lives here.

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
