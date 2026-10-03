use cavalre_ledger_core::*;
use serde_json::Value;
use std::collections::BTreeSet;

fn id(n: usize) -> Id {
    [u8::try_from(n + 1).unwrap(); 32]
}
fn fixture(name: &str) -> Value {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec");
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(base.join(format!("fixtures/{name}.json"))).unwrap(),
    )
    .unwrap();
    let pin: Value =
        serde_json::from_str(&std::fs::read_to_string(base.join("upstream.json")).unwrap())
            .unwrap();
    assert_eq!(fixture["contracts_commit"], pin["references"][0]["commit"]);
    fixture
}

#[test]
fn portable_service_replays_all_162_solidity_hierarchy_actions() {
    let fixture = fixture("hierarchy");
    let mut root = Root {
        id: id(0),
        namespace: id(100),
        kind: RootKind::Journal,
        gross: 0,
    };
    let definitions = fixture["nodes"].as_array().unwrap();
    let mut accounts: Vec<Account> = Vec::new();
    for (i, n) in definitions.iter().enumerate().skip(1) {
        let parent = n["parent"].as_u64().unwrap() as usize;
        let kind = match (
            n["credit"].as_bool().unwrap(),
            n["group"].as_bool().unwrap(),
        ) {
            (false, false) => Kind::DebitLeaf,
            (true, false) => Kind::CreditLeaf,
            (false, true) => Kind::DebitGroup,
            (true, true) => Kind::CreditGroup,
        };
        accounts.push(Account {
            node: Node {
                id: id(i),
                parent: Some(id(parent)),
                kind,
                balances: Balances::default(),
            },
            root: root.id,
            controller: id(i + 50),
            children: 0,
            depth: if parent == 0 {
                1
            } else {
                accounts[parent - 1].depth + 1
            },
        });
    }
    for i in 0..accounts.len() {
        accounts[i].children = accounts
            .iter()
            .filter(|a| a.node.parent == Some(accounts[i].node.id))
            .count() as u32;
    }
    let mut failures = 0;
    for (step_index, step) in fixture["steps"].as_array().unwrap().iter().enumerate() {
        let from = step["from"].as_u64().unwrap() as usize;
        let to = step["to"].as_u64().unwrap() as usize;
        let mut records = Vec::new();
        for endpoint in [from, to] {
            let mut index = endpoint;
            while index != 0 {
                let account = accounts[index - 1];
                if !records.contains(&account) {
                    records.push(account);
                }
                index = usize::from(account.node.parent.unwrap()[0]) - 1;
            }
        }
        let from_index = records.iter().position(|a| a.node.id == id(from)).unwrap();
        let to_index = records.iter().position(|a| a.node.id == id(to)).unwrap();
        let signers = [accounts[from - 1].controller, accounts[to - 1].controller];
        let result = plan_posting(
            &root,
            &records,
            from_index,
            to_index,
            step["amount"].as_u64().unwrap().into(),
            if step["public"].as_bool().unwrap() {
                Operation::TransferJournal
            } else {
                Operation::PostJournal
            },
            Authorization::from_verified_signers(&signers),
        );
        assert_eq!(
            result.is_ok(),
            step["success"].as_bool().unwrap(),
            "step {step_index}: {result:?}"
        );
        if let Ok(plan) = result {
            let mut writes = BTreeSet::new();
            for change in plan.changes() {
                assert!(writes.insert(change.id));
                if change.id == root.id {
                    assert_eq!(change.before, root.node().balances);
                    assert_eq!(change.after.debit, change.after.credit);
                    root.gross = change.after.debit;
                } else {
                    let target = accounts
                        .iter_mut()
                        .find(|a| a.node.id == change.id)
                        .unwrap();
                    assert_eq!(target.node.balances, change.before);
                    target.node.balances = change.after;
                }
            }
        } else {
            failures += 1;
        }
        for (i, node) in std::iter::once(root.node())
            .chain(accounts.iter().map(|a| a.node))
            .enumerate()
        {
            assert_eq!(
                node.balances.debit,
                u128::from(step["balances"][2 * i].as_u64().unwrap()),
                "step {step_index}, node {i}"
            );
            assert_eq!(
                node.balances.credit,
                u128::from(step["balances"][2 * i + 1].as_u64().unwrap()),
                "step {step_index}, node {i}"
            );
            if node.kind.is_group() {
                let children: Vec<_> = accounts
                    .iter()
                    .filter(|a| a.node.parent == Some(node.id))
                    .collect();
                assert_eq!(
                    node.balances.debit,
                    children.iter().map(|a| a.node.balances.debit).sum::<u128>()
                );
                assert_eq!(
                    node.balances.credit,
                    children
                        .iter()
                        .map(|a| a.node.balances.credit)
                        .sum::<u128>()
                );
            }
        }
    }
    assert!(failures > 3);
}

#[test]
fn portable_service_replays_all_138_solidity_custody_actions() {
    let fixture = fixture("custody");
    let mut root = Root {
        id: id(0),
        namespace: id(100),
        kind: RootKind::Asset,
        gross: 0,
    };
    let mut positions: [Account; 2] = std::array::from_fn(|i| {
        open_position(
            &root,
            id(i + 1),
            id(i + 50),
            Authorization::from_verified_signers(&[id(i + 50)]),
        )
        .unwrap()
    });
    let mut wallets = [u128::from(fixture["initial_tokens_per_user"].as_u64().unwrap()); 2];
    let mut vault = 0u128;
    let mut failures = 0;
    for (step_index, step) in fixture["steps"].as_array().unwrap().iter().enumerate() {
        let user = step["user"].as_u64().unwrap() as usize;
        let kind = step["kind"].as_u64().unwrap();
        let amount = u128::from(step["amount"].as_u64().unwrap());
        // A minimal atomic host: external tokens and claim plans commit together.
        let before = TokenBalances {
            vault,
            wallet: wallets[user],
        };
        let result = (|| -> Result<(), Error> {
            let signers = [positions[user].controller];
            let plan = match kind {
                0 => Some(prepare_deposit(
                    &root,
                    &positions[user],
                    amount,
                    before,
                    Authorization::from_verified_signers(&signers),
                )?),
                1 => Some(prepare_withdrawal(
                    &root,
                    &positions[user],
                    amount,
                    before,
                    Authorization::from_verified_signers(&signers),
                )?),
                2 => None,
                _ => panic!("unknown fixture action"),
            };
            let after = if kind == 1 {
                TokenBalances {
                    vault: vault
                        .checked_sub(amount)
                        .ok_or(Error::InsufficientBalance)?,
                    wallet: wallets[user].checked_add(amount).ok_or(Error::Overflow)?,
                }
            } else {
                TokenBalances {
                    vault: vault.checked_add(amount).ok_or(Error::Overflow)?,
                    wallet: wallets[user]
                        .checked_sub(amount)
                        .ok_or(Error::InsufficientBalance)?,
                }
            };
            if let Some(plan) = plan {
                plan.verify_settlement(after)?;
                positions[user].node.balances = plan.position().after;
                root.gross = plan.root().after.debit;
            }
            vault = after.vault;
            wallets[user] = after.wallet;
            Ok(())
        })();
        assert_eq!(
            result.is_ok(),
            step["success"].as_bool().unwrap(),
            "step {step_index}: {result:?}"
        );
        if result.is_err() {
            failures += 1;
        }
        assert_eq!(
            root.gross,
            u128::from(step["total_claims"].as_u64().unwrap())
        );
        assert_eq!(vault, u128::from(step["vault"].as_u64().unwrap()));
        for i in 0..2 {
            assert_eq!(
                positions[i].node.balances.debit,
                u128::from(step["positions"][i].as_u64().unwrap())
            );
            assert_eq!(wallets[i], u128::from(step["wallets"][i].as_u64().unwrap()));
        }
        assert_eq!(
            root.gross,
            positions
                .iter()
                .map(|p| p.node.balances.debit)
                .sum::<u128>()
        );
        assert!(vault >= root.gross);
        assert_eq!(vault + wallets.iter().sum::<u128>(), 20000);
    }
    assert!(failures > 2);
}
