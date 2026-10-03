//! Deterministic hierarchical double-entry posting, independent of storage.
//!
//! This is an arithmetic/topology kernel, NOT authorization. A caller must load
//! authenticated state, authorize the operation and settle dependent rewards
//! before applying a plan. No staking program exists in the current adapter.
#![no_std]

extern crate alloc;
use alloc::vec::Vec;

pub type Id = [u8; 32];
/// Root plus at most 16 edges. A resource bound, not an EVM depth emulation.
pub const MAX_PATH_NODES: usize = 17;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    DebitLeaf,
    CreditLeaf,
    DebitGroup,
    CreditGroup,
}

impl Kind {
    pub const fn is_group(self) -> bool {
        matches!(self, Self::DebitGroup | Self::CreditGroup)
    }
    pub const fn is_credit(self) -> bool {
        matches!(self, Self::CreditLeaf | Self::CreditGroup)
    }
}

/// Gross balances, not lifetime turnover. A group's polarity changes its
/// normal-balance presentation, never the side propagated from a leaf.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Balances {
    pub debit: u128,
    pub credit: u128,
}

impl Balances {
    pub fn normal(self, kind: Kind) -> Result<u128, Error> {
        if kind.is_credit() {
            self.credit.checked_sub(self.debit)
        } else {
            self.debit.checked_sub(self.credit)
        }
        .ok_or(Error::NegativeNormalBalance)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: Id,
    pub parent: Option<Id>,
    pub kind: Kind,
    pub balances: Balances,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub id: Id,
    pub before: Balances,
    pub after: Balances,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPath,
    DepthLimit,
    DifferentRoots,
    ConflictingNode,
    InvalidLeaf,
    InsufficientBalance,
    Overflow,
    NegativeNormalBalance,
    InvalidPublicTransfer,
}

fn validate(path: &[Node]) -> Result<(), Error> {
    if path.len() < 2 {
        return Err(Error::InvalidPath);
    }
    if path.len() > MAX_PATH_NODES {
        return Err(Error::DepthLimit);
    }
    if path[0].kind.is_group() {
        return Err(Error::InvalidLeaf);
    }
    let leaf = path[0];
    if (leaf.kind.is_credit() && leaf.balances.debit != 0)
        || (!leaf.kind.is_credit() && leaf.balances.credit != 0)
    {
        return Err(Error::InvalidLeaf);
    }
    for (i, n) in path.iter().enumerate() {
        if path[..i].iter().any(|prior| prior.id == n.id) {
            return Err(Error::InvalidPath);
        }
        if i > 0 && !n.kind.is_group() {
            return Err(Error::InvalidPath);
        }
        if n.parent != path.get(i + 1).map(|parent| parent.id) {
            return Err(Error::InvalidPath);
        }
    }
    Ok(())
}

/// Produce an all-or-nothing write plan from complete endpoint-to-root paths.
/// Each account appears at most once in the plan. Shared ancestors cancel on
/// the same side; opposite-side changes retain both gross entries.
pub fn plan_posting(from: &[Node], to: &[Node], amount: u128) -> Result<Vec<Change>, Error> {
    validate(from)?;
    validate(to)?;
    if from.last().unwrap().id != to.last().unwrap().id {
        return Err(Error::DifferentRoots);
    }
    for a in from {
        for b in to {
            if a.id == b.id && a != b {
                return Err(Error::ConflictingNode);
            }
        }
    }
    // Internal self-postings are no-ops in LedgerLib. Public transfers use the
    // stricter entry point below, which checks spendable balance first.
    if from[0].id == to[0].id || amount == 0 {
        return Ok(Vec::new());
    }
    let from_credit = from[0].kind.is_credit();
    let to_credit = to[0].kind.is_credit();
    let mut changes = Vec::with_capacity(from.len() + to.len());
    for n in from.iter().chain(to) {
        if changes.iter().any(|change: &Change| change.id == n.id) {
            continue;
        }
        let in_from = from.iter().any(|a| a.id == n.id);
        let in_to = to.iter().any(|a| a.id == n.id);
        // Do not introduce temporary overflow/underflow at a shared ancestor.
        if in_from && in_to && from_credit == to_credit {
            continue;
        }
        let mut after = n.balances;
        if in_from {
            if from_credit {
                after.credit = after.credit.checked_add(amount).ok_or(Error::Overflow)?;
            } else {
                after.debit = after
                    .debit
                    .checked_sub(amount)
                    .ok_or(Error::InsufficientBalance)?;
            }
        }
        if in_to {
            if to_credit {
                after.credit = after
                    .credit
                    .checked_sub(amount)
                    .ok_or(Error::InsufficientBalance)?;
            } else {
                after.debit = after.debit.checked_add(amount).ok_or(Error::Overflow)?;
            }
        }
        changes.push(Change {
            id: n.id,
            before: n.balances,
            after,
        });
    }
    Ok(changes)
}

/// Public presentation grants access only to direct debit leaves. Checking
/// the sender before the internal self-posting no-op matches Ledger.transfer.
/// Signatures/ownership are still the responsibility of the storage adapter.
pub fn plan_public_transfer(
    from: &[Node],
    to: &[Node],
    amount: u128,
) -> Result<Vec<Change>, Error> {
    if from.len() != 2
        || to.len() != 2
        || from[0].kind != Kind::DebitLeaf
        || to[0].kind != Kind::DebitLeaf
    {
        return Err(Error::InvalidPublicTransfer);
    }
    if from[0].balances.debit < amount {
        return Err(Error::InsufficientBalance);
    }
    plan_posting(from, to, amount)
}
