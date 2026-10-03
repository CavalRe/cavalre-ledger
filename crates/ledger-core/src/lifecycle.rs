use crate::{
    Account, Authorization, Balances, Error, Id, Kind, Namespace, Node, Root, RootKind,
    MAX_PATH_NODES,
};

pub fn initialize_namespace(
    id: Id,
    authority: Id,
    auth: Authorization<'_>,
) -> Result<Namespace, Error> {
    auth.require(authority)?;
    Ok(Namespace { id, authority })
}

pub fn create_root(
    namespace: &Namespace,
    id: Id,
    kind: RootKind,
    auth: Authorization<'_>,
) -> Result<Root, Error> {
    auth.require(namespace.authority)?;
    if id == namespace.id {
        return Err(Error::InvalidNode);
    }
    Ok(Root {
        id,
        namespace: namespace.id,
        kind,
        gross: 0,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewNode {
    pub id: Id,
    pub controller: Id,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChildrenChange {
    pub id: Id,
    pub before: u32,
    pub after: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Creation {
    pub account: Account,
    pub parent: Option<ChildrenChange>,
}

/// Parent None means the root. The host must also enforce that the new identity
/// is unused, authenticate its canonical address, and fund any storage rent.
pub fn create_node(
    root: &Root,
    namespace: &Namespace,
    parent: Option<&Account>,
    new: NewNode,
    auth: Authorization<'_>,
) -> Result<Creation, Error> {
    if root.namespace != namespace.id {
        return Err(Error::InvalidNode);
    }
    auth.require(new.controller)?;
    let (parent_id, depth, change) = if let Some(parent) = parent {
        parent.validate(root)?;
        if !parent.node.kind.is_group() {
            return Err(Error::InvalidNode);
        }
        auth.require(parent.controller)?;
        if usize::from(parent.depth) >= MAX_PATH_NODES - 1 {
            return Err(Error::DepthLimit);
        }
        (
            parent.node.id,
            parent.depth + 1,
            Some(ChildrenChange {
                id: parent.node.id,
                before: parent.children,
                after: parent.children.checked_add(1).ok_or(Error::Overflow)?,
            }),
        )
    } else {
        auth.require(namespace.authority)?;
        (root.id, 1, None)
    };
    let account = Account {
        node: Node {
            id: new.id,
            parent: Some(parent_id),
            kind: new.kind,
            balances: Balances::default(),
        },
        root: root.id,
        controller: new.controller,
        children: 0,
        depth,
    };
    account.validate(root)?;
    Ok(Creation {
        account,
        parent: change,
    })
}

/// Holders can open their own direct Asset positions without namespace consent.
pub fn open_position(
    root: &Root,
    id: Id,
    controller: Id,
    auth: Authorization<'_>,
) -> Result<Account, Error> {
    if root.kind != RootKind::Asset {
        return Err(Error::UnsupportedRoot);
    }
    auth.require(controller)?;
    let account = Account {
        node: Node {
            id,
            parent: Some(root.id),
            kind: Kind::DebitLeaf,
            balances: Balances::default(),
        },
        root: root.id,
        controller,
        children: 0,
        depth: 1,
    };
    account.validate(root)?;
    Ok(account)
}

/// Delete only empty, childless accounts, updating the parent's count atomically.
/// The host refunds storage rent to the original payer, never a supplied recipient.
pub fn close_node(
    root: &Root,
    account: &Account,
    parent: Option<&Account>,
    auth: Authorization<'_>,
) -> Result<Option<ChildrenChange>, Error> {
    account.validate(root)?;
    account.check_close(auth)?;
    if let Some(parent) = parent {
        parent.validate(root)?;
        if !parent.node.kind.is_group()
            || account.node.parent != Some(parent.node.id)
            || parent.depth.checked_add(1) != Some(account.depth)
        {
            return Err(Error::InvalidNode);
        }
        Ok(Some(ChildrenChange {
            id: parent.node.id,
            before: parent.children,
            after: parent.children.checked_sub(1).ok_or(Error::InvalidNode)?,
        }))
    } else if account.node.parent == Some(root.id) {
        Ok(None)
    } else {
        Err(Error::InvalidNode)
    }
}
