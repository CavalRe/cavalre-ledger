use cavalre_ledgers_kernel::{
    plan_posting, plan_public_transfer, Balances, Error, Id, Kind, Node, MAX_PATH_NODES,
};
use std::collections::BTreeSet;

fn id(index: usize) -> Id {
    [u8::try_from(index + 1).unwrap(); 32]
}
fn node(index: usize, parent: Option<usize>, kind: Kind, debit: u128, credit: u128) -> Node {
    Node {
        id: id(index),
        parent: parent.map(id),
        kind,
        balances: Balances { debit, credit },
    }
}
fn path(nodes: &[Node], mut index: usize) -> Vec<Node> {
    let mut result = vec![nodes[index]];
    while let Some(parent) = nodes[index].parent {
        index = nodes.iter().position(|n| n.id == parent).unwrap();
        result.push(nodes[index]);
    }
    result
}
fn assert_aggregates(nodes: &[Node]) {
    assert_eq!(nodes[0].balances.debit, nodes[0].balances.credit);
    for group in nodes.iter().filter(|n| n.kind.is_group()) {
        let mut sum = Balances::default();
        for child in nodes.iter().filter(|n| n.parent == Some(group.id)) {
            sum.debit += child.balances.debit;
            sum.credit += child.balances.credit;
        }
        assert_eq!(group.balances, sum);
    }
}

#[test]
fn replays_every_node_from_executed_solidity_hierarchy_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../spec/fixtures/hierarchy.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let pin: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../spec/upstream.json"
        ))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(fixture["contracts_commit"], pin["references"][0]["commit"]);
    let mut nodes: Vec<Node> = fixture["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let kind = match (
                n["credit"].as_bool().unwrap(),
                n["group"].as_bool().unwrap(),
            ) {
                (false, false) => Kind::DebitLeaf,
                (true, false) => Kind::CreditLeaf,
                (false, true) => Kind::DebitGroup,
                (true, true) => Kind::CreditGroup,
            };
            node(
                i,
                (i != 0).then(|| n["parent"].as_u64().unwrap() as usize),
                kind,
                0,
                0,
            )
        })
        .collect();
    let mut rejected = 0;
    for (step_index, step) in fixture["steps"].as_array().unwrap().iter().enumerate() {
        let from = path(&nodes, step["from"].as_u64().unwrap() as usize);
        let to = path(&nodes, step["to"].as_u64().unwrap() as usize);
        let amount = step["amount"].as_u64().unwrap() as u128;
        let result = if step["public"].as_bool().unwrap() {
            plan_public_transfer(&from, &to, amount)
        } else {
            plan_posting(&from, &to, amount)
        };
        assert_eq!(
            result.is_ok(),
            step["success"].as_bool().unwrap(),
            "step {step_index}: {result:?}"
        );
        match result {
            Ok(plan) => {
                let mut written = BTreeSet::new();
                for change in plan {
                    assert!(written.insert(change.id), "ancestor scheduled twice");
                    let target = nodes.iter_mut().find(|n| n.id == change.id).unwrap();
                    assert_eq!(target.balances, change.before);
                    target.balances = change.after;
                }
            }
            Err(_) => rejected += 1,
        }
        for (i, n) in nodes.iter().enumerate() {
            assert_eq!(
                n.balances.debit,
                step["balances"][2 * i].as_u64().unwrap() as u128,
                "step {step_index}, node {i}, debit"
            );
            assert_eq!(
                n.balances.credit,
                step["balances"][2 * i + 1].as_u64().unwrap() as u128,
                "step {step_index}, node {i}, credit"
            );
        }
        assert_aggregates(&nodes);
    }
    assert!(
        rejected > 3,
        "fixture must include internal and public failures"
    );
}

#[test]
fn shared_ancestors_cancel_on_same_side_and_retain_gross_opposite_sides() {
    let root = node(0, None, Kind::DebitGroup, 100, 100);
    let group = node(1, Some(0), Kind::CreditGroup, 50, 30);
    let a = node(2, Some(1), Kind::DebitLeaf, 40, 0);
    let b = node(3, Some(1), Kind::DebitLeaf, 10, 0);
    let c = node(4, Some(1), Kind::CreditLeaf, 0, 30);
    let plan = plan_posting(&[a, group, root], &[b, group, root], 7).unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].after.debit, 33);
    assert_eq!(plan[1].after.debit, 17);
    let plan = plan_posting(&[c, group, root], &[a, group, root], 7).unwrap();
    let group_after = plan.iter().find(|p| p.id == group.id).unwrap().after;
    let root_after = plan.iter().find(|p| p.id == root.id).unwrap().after;
    assert_eq!(
        group_after,
        Balances {
            debit: 57,
            credit: 37
        }
    );
    assert_eq!(
        root_after,
        Balances {
            debit: 107,
            credit: 107
        }
    );
    assert_eq!(
        group_after.debit - group_after.credit,
        group.balances.debit - group.balances.credit
    );
}

#[test]
fn public_self_transfer_checks_funds_and_rejects_custody_groups_and_credit_leaves() {
    let root = node(0, None, Kind::DebitGroup, 10, 10);
    let a = node(1, Some(0), Kind::DebitLeaf, 10, 0);
    assert!(plan_posting(&[a, root], &[a, root], 11).unwrap().is_empty());
    assert_eq!(
        plan_public_transfer(&[a, root], &[a, root], 11),
        Err(Error::InsufficientBalance)
    );
    assert!(plan_public_transfer(&[a, root], &[a, root], 10)
        .unwrap()
        .is_empty());
    let group = node(2, Some(0), Kind::DebitGroup, 10, 0);
    let nested = node(3, Some(2), Kind::DebitLeaf, 10, 0);
    let source = node(4, Some(0), Kind::CreditLeaf, 0, 10);
    for from in [
        vec![group, root],
        vec![nested, group, root],
        vec![source, root],
    ] {
        assert_eq!(
            plan_public_transfer(&from, &[a, root], 0),
            Err(Error::InvalidPublicTransfer)
        );
    }
}

#[test]
fn malformed_missing_repeated_and_conflicting_paths_are_rejected() {
    let root = node(0, None, Kind::DebitGroup, 10, 10);
    let group = node(1, Some(0), Kind::DebitGroup, 10, 0);
    let a = node(2, Some(1), Kind::DebitLeaf, 10, 0);
    let b = node(3, Some(0), Kind::DebitLeaf, 0, 0);
    assert_eq!(plan_posting(&[], &[b, root], 1), Err(Error::InvalidPath));
    assert_eq!(
        plan_posting(&[a, root], &[b, root], 1),
        Err(Error::InvalidPath)
    );
    assert_eq!(
        plan_posting(&[a, group, group, root], &[b, root], 1),
        Err(Error::InvalidPath)
    );
    let other_root = node(4, None, Kind::DebitGroup, 10, 10);
    let foreign = node(5, Some(4), Kind::DebitLeaf, 0, 0);
    assert_eq!(
        plan_posting(&[a, group, root], &[foreign, other_root], 1),
        Err(Error::DifferentRoots)
    );
    let mut conflict = root;
    conflict.balances.debit = 999;
    assert_eq!(
        plan_posting(&[a, group, root], &[b, conflict], 1),
        Err(Error::ConflictingNode)
    );
    let mut invalid_leaf = a;
    invalid_leaf.balances.credit = 1;
    assert_eq!(
        plan_posting(&[invalid_leaf, group, root], &[b, root], 1),
        Err(Error::InvalidLeaf)
    );
}

#[test]
fn checks_wide_bounds_without_partial_mutation_or_unbounded_paths() {
    let root = node(0, None, Kind::DebitGroup, u128::MAX, u128::MAX);
    let source = node(1, Some(0), Kind::CreditLeaf, 0, u128::MAX);
    let holder = node(2, Some(0), Kind::DebitLeaf, 0, 0);
    assert_eq!(
        plan_posting(&[source, root], &[holder, root], 1),
        Err(Error::Overflow)
    );
    assert_eq!(
        plan_posting(&[holder, root], &[source, root], 1),
        Err(Error::InsufficientBalance)
    );
    let mut deep: Vec<Node> = (0..MAX_PATH_NODES + 1)
        .map(|i| {
            node(
                i,
                Some(i + 1),
                if i == 0 {
                    Kind::DebitLeaf
                } else {
                    Kind::DebitGroup
                },
                0,
                0,
            )
        })
        .collect();
    deep.last_mut().unwrap().parent = None;
    assert_eq!(
        plan_posting(&deep, &[holder, root], 0),
        Err(Error::DepthLimit)
    );
}
