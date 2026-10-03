use crate::{
    Account, Authorization, Change, Error, Id, Kind, Node, Root, RootKind, MAX_PATH_NODES,
};
use alloc::vec::Vec;

pub const MAX_RECORDS: usize = 2 * (MAX_PATH_NODES - 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Both controllers consent; debit and credit leaves, including nested ones.
    PostJournal,
    /// Source consent; direct Journal debit leaves only.
    TransferJournal,
    /// Source consent; direct or nested Asset debit claims only.
    TransferClaims,
}

impl Operation {
    /// Adapters may check this before decoding root-specific record layouts.
    pub fn validate_root(self, kind: RootKind) -> Result<(), Error> {
        if (self == Self::TransferClaims) != (kind == RootKind::Asset) {
            Err(Error::UnsupportedRoot)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub id: Id,
    pub custody: Id,
    pub credit: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posting {
    changes: Vec<Change>,
    pub from: Endpoint,
    pub to: Endpoint,
}

impl Posting {
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }
}

/// Validate the exact unique union of both ancestry paths and authorize the
/// selected operation, even for zero amounts and self-postings. Records must be
/// current authenticated state, not client-supplied snapshots. Nothing is mutated.
pub fn plan_posting(
    root: &Root,
    records: &[Account],
    from: usize,
    to: usize,
    amount: u128,
    operation: Operation,
    authorization: Authorization<'_>,
) -> Result<Posting, Error> {
    operation.validate_root(root.kind)?;
    if records.is_empty() || records.len() > MAX_RECORDS {
        return Err(Error::InvalidAccounts);
    }
    for (i, record) in records.iter().enumerate() {
        record.validate(root)?;
        if records[..i].iter().any(|r| r.node.id == record.node.id) {
            return Err(Error::InvalidAccounts);
        }
    }
    let mut used = [false; MAX_RECORDS];
    let from_path = path(root, records, from, &mut used)?;
    let to_path = path(root, records, to, &mut used)?;
    if !used[..records.len()].iter().all(|u| *u) {
        return Err(Error::InvalidAccounts);
    }
    authorization.require(records[from].controller)?;
    let changes = match operation {
        Operation::PostJournal => {
            authorization.require(records[to].controller)?;
            cavalre_ledgers_kernel::plan_posting(&from_path, &to_path, amount)?
        }
        Operation::TransferJournal => {
            cavalre_ledgers_kernel::plan_public_transfer(&from_path, &to_path, amount)?
        }
        Operation::TransferClaims => {
            if from_path[0].kind != Kind::DebitLeaf || to_path[0].kind != Kind::DebitLeaf {
                return Err(Error::Accounting(
                    cavalre_ledgers_kernel::Error::InvalidLeaf,
                ));
            }
            if from_path[0].balances.debit < amount {
                return Err(Error::InsufficientBalance);
            }
            cavalre_ledgers_kernel::plan_posting(&from_path, &to_path, amount)?
        }
    };
    let endpoint = |path: &[Node]| Endpoint {
        id: path[0].id,
        custody: path[path.len() - 2].id,
        credit: path[0].kind.is_credit(),
    };
    Ok(Posting {
        changes,
        from: endpoint(&from_path),
        to: endpoint(&to_path),
    })
}

fn path(
    root: &Root,
    records: &[Account],
    start: usize,
    used: &mut [bool; MAX_RECORDS],
) -> Result<Vec<Node>, Error> {
    let mut path = Vec::with_capacity(MAX_PATH_NODES);
    let mut index = start;
    loop {
        let record = records.get(index).ok_or(Error::InvalidAccounts)?;
        if path.len() >= MAX_PATH_NODES - 1 {
            return Err(Error::DepthLimit);
        }
        if path.iter().any(|n: &Node| n.id == record.node.id) {
            return Err(Error::InvalidNode);
        }
        used[index] = true;
        path.push(record.node);
        if record.node.parent == Some(root.id) {
            break;
        }
        index = records
            .iter()
            .position(|r| Some(r.node.id) == record.node.parent)
            .ok_or(Error::InvalidAccounts)?;
        if !records[index].node.kind.is_group()
            || records[index].depth.checked_add(1) != Some(record.depth)
        {
            return Err(Error::InvalidNode);
        }
    }
    path.push(root.node());
    Ok(path)
}
