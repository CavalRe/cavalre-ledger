use cavalre_ledger_core::ledger_lib::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Memory {
    flags: BTreeMap<u64, Flags<u64>>,
    balances: BTreeMap<u64, Balances>,
    custody: BTreeMap<u64, u64>,
}
impl Store<u64> for Memory {
    fn flags(&self, id: &u64) -> Result<Option<Flags<u64>>, Error> {
        Ok(self.flags.get(id).copied())
    }
    fn custody_account(&self, id: &u64) -> Result<Option<u64>, Error> {
        Ok(self.custody.get(id).copied())
    }
    fn relative(&self, id: &u64) -> Result<u64, Error> {
        Ok(id % 256)
    }
}
impl AccountingStore<u64> for Memory {
    fn balances(&self, id: &u64) -> Result<Balances, Error> {
        Ok(self.balances.get(id).copied().unwrap_or_default())
    }
}
struct Addresses;
impl AddressDerivation<u64> for Addresses {
    fn to_address(&self, parent: &u64, relative: &u64) -> u64 {
        parent * 256 + relative
    }
}
fn endpoint(store: &Memory, id: u64) -> Endpoint<u64> {
    Endpoint {
        relative: id % 256,
        flags: store.flags[&id],
    }
}
fn commit(store: &mut Memory, changes: Vec<BalanceChange<u64>>) {
    for change in changes {
        assert_eq!(store.balances(&change.absolute).unwrap(), change.before);
        store.balances.insert(change.absolute, change.after);
    }
}

#[test]
fn replays_all_original_solidity_hierarchy_cases() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../spec/fixtures/hierarchy.json")).unwrap();
    let mut store = Memory::default();
    let mut ids = vec![1];
    for (i, node) in fixture["nodes"].as_array().unwrap().iter().enumerate() {
        let parent = if i == 0 {
            0
        } else {
            ids[node["parent"].as_u64().unwrap() as usize]
        };
        let id = if i == 0 {
            1
        } else {
            Addresses.to_address(&parent, &(i as u64 + 1))
        };
        if i != 0 {
            ids.push(id);
        }
        let kind = match (
            node["credit"].as_bool().unwrap(),
            node["group"].as_bool().unwrap(),
        ) {
            (false, false) => AccountKind::DebitLedger,
            (true, false) => AccountKind::CreditLedger,
            (false, true) => AccountKind::DebitGroup,
            (true, true) => AccountKind::CreditGroup,
        };
        let depth = if i == 0 {
            2
        } else {
            store.flags[&parent].depth + 1
        };
        store.flags.insert(
            id,
            Flags {
                parent,
                account_kind: kind,
                token_kind: if i == 0 {
                    TokenKind::Internal
                } else {
                    TokenKind::Unregistered
                },
                depth,
            },
        );
    }
    let steps = fixture["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 162);
    for (index, step) in steps.iter().enumerate() {
        let from = ids[step["from"].as_u64().unwrap() as usize];
        let to = ids[step["to"].as_u64().unwrap() as usize];
        let amount = u128::from(step["amount"].as_u64().unwrap());
        let result = if step["public"].as_bool().unwrap()
            && (store.flags[&from].depth != 3
                || store.flags[&to].depth != 3
                || store.flags[&from].account_kind != AccountKind::DebitLedger
                || store.flags[&to].account_kind != AccountKind::DebitLedger
                || store.balances(&from).unwrap().debit < amount)
        {
            Err(Error::InvalidLedgerAccount)
        } else {
            transfer(
                &store,
                &Addresses,
                &1,
                endpoint(&store, from),
                endpoint(&store, to),
                amount,
            )
        };
        assert_eq!(
            result.is_ok(),
            step["success"].as_bool().unwrap(),
            "step {index}: {result:?}"
        );
        if let Ok(changes) = result {
            commit(&mut store, changes);
        }
        for (i, id) in ids.iter().enumerate() {
            let actual = store.balances(id).unwrap();
            assert_eq!(
                actual.debit,
                u128::from(step["balances"][i * 2].as_u64().unwrap()),
                "step {index}, node {i}"
            );
            assert_eq!(
                actual.credit,
                u128::from(step["balances"][i * 2 + 1].as_u64().unwrap()),
                "step {index}, node {i}"
            );
        }
    }
}

#[test]
fn shared_ancestor_cancels_without_overflow_and_self_transfer_checks_funds() {
    let mut store = Memory::default();
    store.flags.insert(
        1,
        Flags {
            parent: 0,
            account_kind: AccountKind::DebitGroup,
            token_kind: TokenKind::External,
            depth: 2,
        },
    );
    store.flags.insert(
        258,
        Flags {
            parent: 1,
            account_kind: AccountKind::DebitGroup,
            token_kind: TokenKind::Unregistered,
            depth: 3,
        },
    );
    store.custody.insert(258, 258);
    let from = Endpoint {
        relative: 3,
        flags: Flags {
            parent: 258,
            account_kind: AccountKind::DebitLedger,
            token_kind: TokenKind::Unregistered,
            depth: 4,
        },
    };
    let to = Endpoint {
        relative: 4,
        ..from
    };
    store.balances.insert(
        258,
        Balances {
            debit: u128::MAX,
            credit: 0,
        },
    );
    store.balances.insert(
        Addresses.to_address(&258, &3),
        Balances {
            debit: 9,
            credit: 0,
        },
    );
    let changes = transfer_debits(&store, &Addresses, &1, from, to, &2, 9).unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|c| c.absolute != 258 && c.absolute != 1));
    assert_eq!(
        transfer_debits(&store, &Addresses, &1, from, from, &2, 10),
        Err(Error::InsufficientBalance)
    );
    assert_eq!(
        transfer_debits(&store, &Addresses, &1, from, to, &7, 1),
        Err(Error::Unauthorized)
    );
}
