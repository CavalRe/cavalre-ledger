use crate::{Account, Authorization, Change, Error, Kind, Root, RootKind};

/// Fresh observations of authenticated native custody accounts, in base units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenBalances {
    pub vault: u128,
    pub wallet: u128,
}

/// Preflighted claim changes. Deposit writes may be committed only after exact
/// receipt is verified. Withdrawal writes precede external transfer; the host
/// must roll back ALL effects if transfer or verification fails. Plans cannot
/// outlive their transaction or be replayed against a different state snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyPlan {
    position: Change,
    root: Change,
    before: TokenBalances,
    amount: u128,
    deposit: bool,
}

impl CustodyPlan {
    pub fn position(&self) -> &Change {
        &self.position
    }
    pub fn root(&self) -> &Change {
        &self.root
    }

    pub fn verify_settlement(&self, after: TokenBalances) -> Result<(), Error> {
        let (vault_delta, wallet_delta) = if self.deposit {
            (
                after.vault.checked_sub(self.before.vault),
                self.before.wallet.checked_sub(after.wallet),
            )
        } else {
            (
                self.before.vault.checked_sub(after.vault),
                after.wallet.checked_sub(self.before.wallet),
            )
        };
        if vault_delta == Some(self.amount) && wallet_delta == Some(self.amount) {
            Ok(())
        } else {
            Err(Error::UnexpectedTokenDelta)
        }
    }
}

pub fn prepare_deposit(
    root: &Root,
    position: &Account,
    amount: u128,
    before: TokenBalances,
    auth: Authorization<'_>,
) -> Result<CustodyPlan, Error> {
    prepare(root, position, amount, before, auth, true)
}

pub fn prepare_withdrawal(
    root: &Root,
    position: &Account,
    amount: u128,
    before: TokenBalances,
    auth: Authorization<'_>,
) -> Result<CustodyPlan, Error> {
    prepare(root, position, amount, before, auth, false)
}

fn prepare(
    root: &Root,
    position: &Account,
    amount: u128,
    before: TokenBalances,
    auth: Authorization<'_>,
    deposit: bool,
) -> Result<CustodyPlan, Error> {
    if root.kind != RootKind::Asset {
        return Err(Error::UnsupportedRoot);
    }
    position.validate(root)?;
    if position.node.kind != Kind::DebitLeaf || position.depth != 1 {
        return Err(Error::InvalidNode);
    }
    auth.require(position.controller)?;
    if !deposit && before.vault < root.gross {
        return Err(Error::Undercollateralized);
    }
    let balance = position.node.balances.debit;
    let (balance, gross) = if deposit {
        (
            balance.checked_add(amount).ok_or(Error::Overflow)?,
            root.gross.checked_add(amount).ok_or(Error::Overflow)?,
        )
    } else {
        (
            balance
                .checked_sub(amount)
                .ok_or(Error::InsufficientBalance)?,
            root.gross.checked_sub(amount).ok_or(Error::Overflow)?,
        )
    };
    let mut position_change = Change {
        id: position.node.id,
        before: position.node.balances,
        after: position.node.balances,
    };
    position_change.after.debit = balance;
    let mut root_change = Change {
        id: root.id,
        before: root.node().balances,
        after: root.node().balances,
    };
    root_change.after.debit = gross;
    root_change.after.credit = gross;
    Ok(CustodyPlan {
        position: position_change,
        root: root_change,
        before,
        amount,
        deposit,
    })
}
