//! Corresponds to cavalre-contracts/modules/ledger/LedgerLib.sol.
//! These read-only helpers consume authenticated state, never instruction-supplied
//! flags. The program account loader will implement Store; this is not a wire layout.
use anchor_lang::prelude::Pubkey;

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

/// Decoded Solidity flags. Solana uses full public keys rather than a packed
/// 160-bit parent. None in Store::flags represents unregistered metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags {
    pub parent: Pubkey,
    pub account_kind: AccountKind,
    pub token_kind: TokenKind,
    pub depth: u8,
}

/// Read boundary corresponding to the original flags, custody and subaccount
/// mappings. Missing metadata does NOT imply a zero balance or missing storage.
/// Implementations must authenticate ownership, account identity and decoding;
/// malformed stored data must return an error, not None.
pub trait Store {
    fn flags(&self, absolute: &Pubkey) -> Result<Option<Flags>, Error>;
    fn custody_account(&self, absolute: &Pubkey) -> Result<Option<Pubkey>, Error>;
    fn relative(&self, absolute: &Pubkey) -> Result<Pubkey, Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidAccountGroup,
    DifferentRoots,
    InvalidAddress,
    DepthOverflow,
}

/// Same parent/relative identity relationship as toAddress, using a Solana PDA.
/// Derivation neither allocates storage nor registers an account.
pub fn to_address(program: &Pubkey, parent: &Pubkey, relative: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"account", parent.as_ref(), relative.as_ref()], program)
}

/// Roots identify themselves; registered descendants resolve through custody.
/// An implicit leaf requires its parent context, exactly as in LedgerLib.
pub fn ledger(store: &impl Store, absolute: &Pubkey) -> Result<Option<Pubkey>, Error> {
    if let Some(custodian) = store.custody_account(absolute)? {
        let flags = store.flags(&custodian)?.ok_or(Error::InvalidAddress)?;
        return Ok(Some(flags.parent));
    }
    Ok(store.flags(absolute)?.and_then(|flags| {
        (flags.depth == 2 && flags.account_kind.is_group()).then_some(*absolute)
    }))
}

/// Returns (effective flags, original registered flags, absolute address).
/// Unregistered leaves inherit their parent's polarity and next depth without
/// registering the leaf, changing custody, or requiring the receiver to sign.
pub fn effective_flags(
    store: &impl Store,
    program: &Pubkey,
    ledger_address: &Pubkey,
    parent: &Pubkey,
    relative: &Pubkey,
) -> Result<(Flags, Option<Flags>, Pubkey), Error> {
    let parent_flags = store.flags(parent)?.ok_or(Error::InvalidAccountGroup)?;
    if !parent_flags.account_kind.is_group() {
        return Err(Error::InvalidAccountGroup);
    }
    if ledger(store, parent)? != Some(*ledger_address) {
        return Err(Error::DifferentRoots);
    }
    let (absolute, _) = to_address(program, parent, relative);
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
pub fn custody(
    store: &impl Store,
    ledger_address: &Pubkey,
    effective: Flags,
    relative: &Pubkey,
) -> Result<(Pubkey, bool), Error> {
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
