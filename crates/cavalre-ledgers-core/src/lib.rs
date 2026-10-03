//! Portable Ledger service rules over authenticated state.
//!
//! The host authenticates identities and signers, loads current records, and
//! atomically applies successful plans. This crate owns accounting, topology,
//! controller authorization and custody invariants; it does not verify signatures,
//! derive addresses, serialize storage, transfer native tokens or settle rewards.
//! See `docs/LEDGER_CORE.md` for the adapter contract.
#![no_std]

extern crate alloc;

mod custody;
mod hierarchy;
mod lifecycle;

pub use cavalre_ledgers_kernel::{Balances, Change, Id, Kind, Node, MAX_PATH_NODES};
pub use custody::{prepare_deposit, prepare_withdrawal, CustodyPlan, TokenBalances};
pub use hierarchy::{plan_posting, Endpoint, Operation, Posting, MAX_RECORDS};
pub use lifecycle::{
    close_node, create_node, create_root, initialize_namespace, open_position, ChildrenChange,
    Creation, NewNode,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unauthorized,
    InvalidNode,
    InvalidAccounts,
    UnsupportedRoot,
    NonemptyNode,
    DepthLimit,
    Overflow,
    InsufficientBalance,
    Undercollateralized,
    UnexpectedTokenDelta,
    Accounting(cavalre_ledgers_kernel::Error),
}

impl From<cavalre_ledgers_kernel::Error> for Error {
    fn from(error: cavalre_ledgers_kernel::Error) -> Self {
        match error {
            cavalre_ledgers_kernel::Error::InsufficientBalance => Self::InsufficientBalance,
            cavalre_ledgers_kernel::Error::Overflow => Self::Overflow,
            cavalre_ledgers_kernel::Error::DepthLimit => Self::DepthLimit,
            other => Self::Accounting(other),
        }
    }
}

/// Only identities authenticated by the host for this invocation belong here.
/// Never construct this from a transaction's unverified list of requested signers.
#[derive(Clone, Copy, Debug)]
pub struct Authorization<'a>(&'a [Id]);

impl<'a> Authorization<'a> {
    pub const fn from_verified_signers(signers: &'a [Id]) -> Self {
        Self(signers)
    }

    pub fn require(self, controller: Id) -> Result<(), Error> {
        if self.0.contains(&controller) {
            Ok(())
        } else {
            Err(Error::Unauthorized)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Namespace {
    pub id: Id,
    pub authority: Id,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootKind {
    /// Fully backed native claims. Credit issuance is confined to custody.
    Asset,
    /// Internal double-entry units with no native redemption route.
    Journal,
}

/// One authoritative gross supply; root D, root C and an Asset's implicit Source
/// credit are derived from it. Native vault balances are never cached here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    pub id: Id,
    pub namespace: Id,
    pub kind: RootKind,
    pub gross: u128,
}

impl Root {
    pub const fn node(self) -> Node {
        Node {
            id: self.id,
            parent: None,
            kind: Kind::DebitGroup,
            balances: Balances {
                debit: self.gross,
                credit: self.gross,
            },
        }
    }
}

/// A view of one stored account, never an additional balance store. Identity,
/// root, parent, controller and kind are immutable for the account's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Account {
    pub node: Node,
    pub root: Id,
    pub controller: Id,
    pub children: u32,
    pub depth: u8,
}

impl Account {
    /// Closure authorization for a host-authenticated account. Nested closure
    /// must additionally call `close_node` to validate and update its parent.
    pub fn check_close(self, auth: Authorization<'_>) -> Result<(), Error> {
        auth.require(self.controller)?;
        if self.node.balances != Balances::default() || self.children != 0 {
            return Err(Error::NonemptyNode);
        }
        Ok(())
    }

    pub fn validate(self, root: &Root) -> Result<(), Error> {
        if self.root != root.id
            || self.node.id == root.id
            || self.node.parent.is_none()
            || self.node.parent == Some(self.node.id)
        {
            return Err(Error::InvalidNode);
        }
        if self.depth == 0 || usize::from(self.depth) >= MAX_PATH_NODES {
            return Err(Error::DepthLimit);
        }
        if (self.node.parent == Some(root.id)) != (self.depth == 1) {
            return Err(Error::InvalidNode);
        }
        if !self.node.kind.is_group()
            && (self.children != 0
                || if self.node.kind.is_credit() {
                    self.node.balances.debit != 0
                } else {
                    self.node.balances.credit != 0
                })
        {
            return Err(Error::InvalidNode);
        }
        if root.kind == RootKind::Asset
            && (self.node.kind == Kind::CreditLeaf || self.node.balances.credit != 0)
        {
            return Err(Error::UnsupportedRoot);
        }
        Ok(())
    }
}
