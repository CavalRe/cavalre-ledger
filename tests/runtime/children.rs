//! Maintained child vectors and reverse indexes, matching Solidity subs/subIndex.
use super::*;
use ledger::ledger_view::Reader;

fn page(h: &Harness, root: Address, start: u32, limit: u32) -> Vec<Address> {
    let mut reader = Reader::new();
    let record = h.svm.get_account(&h.storage(root)).unwrap();
    reader
        .insert(ap(h.storage(root)), &ap(record.owner), &record.data)
        .unwrap();
    reader
        .sub_accounts(&ap(root), &ap(root), start as usize, limit as usize)
        .unwrap()
        .into_iter()
        .map(sa)
        .collect()
}

#[test]
fn inline_children_preserve_insertion_order_swap_pop_and_reuse_without_sibling_reads() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let values = [50, 20, 90, 10].map(|n| Address::new_from_array([n; 32]));
    for relative in values {
        let i = leaf(&h, root, h.key(0), root, relative, "Leaf", false);
        succeeds(&mut h, &[0], i);
    }
    assert_eq!(
        page(&h, root, 0, 10),
        [vec![sa(SOURCE)], values.to_vec()].concat()
    );
    assert_eq!(page(&h, root, 2, 2), values[1..3]);
    let i = remove(&h, root, h.key(0), root, values[1], false);
    succeeds(&mut h, &[0], i);
    assert_eq!(
        page(&h, root, 0, 10),
        vec![sa(SOURCE), values[0], values[3], values[2]]
    );
    assert_eq!(h.record(child(root, values[3])).child_index, 3);
    assert!(h.svm.get_account(&child(root, values[1])).is_none());
    assert!(h
        .svm
        .get_account(&sa(ledger::ledger_lib::metadata_address(
            &ap(root),
            &ap(values[1])
        )
        .0))
        .is_none());
    assert_eq!(h.record(root).children.len(), 4);
    let i = leaf(&h, root, h.key(0), root, values[1], "Leaf", false);
    succeeds(&mut h, &[0], i);
    assert_eq!(page(&h, root, 4, 1), vec![values[1]]);
    assert_eq!(h.record(child(root, values[1])).child_index, 5);
    let i = remove(&h, root, h.key(0), root, values[1], false);
    succeeds(&mut h, &[0], i); // Last-child removal needs no other child record.
    assert_eq!(
        page(&h, root, 0, 10),
        vec![sa(SOURCE), values[0], values[3], values[2]]
    );
}

#[test]
fn missing_readonly_or_forged_inline_children_and_missing_swap_child_roll_back() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let a = Address::new_from_array([70; 32]);
    let b = Address::new_from_array([71; 32]);
    for relative in [a, b] {
        let i = leaf(&h, root, h.key(0), root, relative, "Leaf", false);
        succeeds(&mut h, &[0], i);
    }
    let valid = h.indexed(remove(&h, root, h.key(0), root, a, false));
    for (target, readonly) in [
        (root, true),
        (child(root, b), false),
        (h.metadata_key(child(root, a)), false),
    ] {
        let mut invalid = valid.clone();
        if readonly {
            invalid
                .accounts
                .iter_mut()
                .find(|m| m.pubkey == target)
                .unwrap()
                .is_writable = false;
        } else {
            invalid.accounts.retain(|m| m.pubkey != target);
        }
        let before: Vec<_> = valid
            .accounts
            .iter()
            .map(|m| (m.pubkey, h.svm.get_account(&m.pubkey)))
            .collect();
        let failure = run_raw(&mut h, &[0], invalid).expect_err("invalid index inputs must reject");
        for (key, old) in before {
            let mut current = h.svm.get_account(&key);
            if key == h.key(0) {
                current.as_mut().unwrap().lamports += failure.meta.fee;
            }
            assert_eq!(current, old, "partial child-index mutation at {key}");
        }
    }
    // A corrupt reverse index cannot point at another parent's relative entry.
    let target = child(root, a);
    let original = h.svm.get_account(&target).unwrap();
    let mut forged = original.clone();
    let offset = ledger::ledger_storage::CHILD_INDEX;
    forged.data[offset..offset + 4].copy_from_slice(&3u32.to_le_bytes());
    h.svm.set_account(target, forged).unwrap();
    assert!(run_raw(&mut h, &[0], valid.clone()).is_err());
    h.svm.set_account(target, original).unwrap();
    assert_eq!(page(&h, root, 0, 10), vec![sa(SOURCE), a, b]);
    assert!(run_raw(&mut h, &[0], valid).is_ok());
    assert_eq!(page(&h, root, 0, 10), vec![sa(SOURCE), b]);
}

#[test]
fn structural_writes_preserve_large_child_arrays_without_copying_siblings() {
    let mut costs = Vec::new();
    for count in [8u32, 256, 2_048, 4_096] {
        let mut h = Harness::new();
        let (root, _) = h.internal();
        let existing = Address::new_from_array([72; 32]);
        let created = Address::new_from_array([73; 32]);
        let received = Address::new_from_array([74; 32]);
        let call = leaf(&h, root, h.key(0), root, existing, "", false);
        succeeds(&mut h, &[0], call);

        // Seed untouched sibling slots offchain. Their individual accounts are
        // deliberately omitted: each operation needs only its affected leaves.
        let mut parent = h.svm.get_account(&root).unwrap();
        let mut record = h.record(root);
        for n in 2..count {
            let mut bytes = [91; 32];
            bytes[..4].copy_from_slice(&n.to_le_bytes());
            record.children.push(ap(Address::new_from_array(bytes)));
        }
        parent.data = record.to_bytes().unwrap();
        parent.lamports = h.svm.minimum_balance_for_rent_exemption(parent.data.len());
        h.svm.set_account(root, parent.clone()).unwrap();
        let offset = ledger::ledger_storage::children_offset(2) + 4;
        let original_slots = parent.data[offset..].to_vec();

        let add = h.indexed(leaf(&h, root, h.key(0), root, created, "", false));
        let added = run_raw(&mut h, &[0], add).unwrap();
        let after = h.svm.get_account(&root).unwrap();
        assert_eq!(&after.data[offset..parent.data.len()], original_slots);
        assert_eq!(after.data.len(), parent.data.len() + 32);
        assert_eq!(h.record(child(root, created)).child_index, count + 1);

        // Swap the new last child into the existing child's slot; retain every
        // other sibling byte and update only the moved child's reverse index.
        let remove = h.indexed(remove(&h, root, h.key(0), root, existing, false));
        let removed = run_raw(&mut h, &[0], remove).unwrap();
        let after = h.svm.get_account(&root).unwrap();
        let mut expected = original_slots;
        expected[32..64].copy_from_slice(created.as_ref());
        assert_eq!(&after.data[offset..], expected);
        assert_eq!(after.data.len(), parent.data.len());
        assert_eq!(h.record(child(root, created)).child_index, 2);
        assert!(h.svm.get_account(&child(root, existing)).is_none());

        // First receipt exercises materialization in commit, rather than Add.
        let receipt = h.indexed(acceptance::transfer(
            &h,
            root,
            h.key(0),
            (root, sa(SOURCE)),
            (root, received),
            7,
            &[],
        ));
        let receipt = run(&mut h, &[0], receipt).unwrap();
        let after = h.svm.get_account(&root).unwrap();
        assert_eq!(&after.data[offset..parent.data.len()], expected);
        assert_eq!(h.record(child(root, received)).child_index, count + 1);
        assert_eq!(h.record(child(root, received)).debit, 7);
        assert_eq!((h.record(root).debit, h.record(root).credit), (7, 7));
        let row = [
            added.compute_units_consumed,
            removed.compute_units_consumed,
            receipt.compute_units_consumed,
        ];
        eprintln!(
            "CHILD_SLOT_CU children={count} add={} remove={} first_receipt={}",
            row[0], row[1], row[2]
        );
        costs.push(row);
    }
    // Allow runtime account-size/CPI overhead, but reject sibling decoding or
    // serialization proportional to thousands of child identities.
    for operation in 0..3 {
        let min = costs.iter().map(|row| row[operation]).min().unwrap();
        let max = costs.iter().map(|row| row[operation]).max().unwrap();
        assert!(max - min < 5_000, "child-count CU regression: {costs:?}");
    }
}
