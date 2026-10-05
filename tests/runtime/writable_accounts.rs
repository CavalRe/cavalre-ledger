//! Minimal client write declarations, checked against actual sBPF execution.
use super::*;
use ledger::{ledger_lib::Child, ledger_view::Reader};

fn endpoint(parent: Address, relative: Address) -> Child {
    Child {
        parent: ap(parent),
        relative: ap(relative),
    }
}

fn planned(h: &Harness, mut ix: Instruction) -> Instruction {
    let mut reader = Reader::new();
    // LedgerAccounts has payer, authority, root, System, then remaining records.
    for meta in ix.accounts.iter().skip(2).filter(|m| m.pubkey != SYSTEM) {
        match h.svm.get_account(&meta.pubkey) {
            Some(account) => reader
                .insert(ap(meta.pubkey), &ap(account.owner), &account.data)
                .unwrap(),
            None => reader.insert_missing(ap(meta.pubkey)).unwrap(),
        }
    }
    let data = &ix.data;
    let key = |offset| Address::new_from_array(data[offset..offset + 32].try_into().unwrap());
    let writable = reader
        .transfer_writable_accounts(
            &ap(ix.accounts[2].pubkey),
            endpoint(key(8), key(40)),
            endpoint(key(72), key(104)),
            u128::from_le_bytes(data[136..152].try_into().unwrap()),
        )
        .unwrap();
    for meta in ix.accounts.iter_mut().skip(2) {
        meta.is_writable = writable.contains(&ap(meta.pubkey));
    }
    ix
}

fn readonly(ix: &mut Instruction, key: Address) {
    for meta in &mut ix.accounts {
        if meta.pubkey == key {
            meta.is_writable = false;
        }
    }
}

fn check_plan(h: &mut Harness, ix: Instruction, expected: &[Address]) {
    let mut actual: Vec<_> = ix
        .accounts
        .iter()
        .skip(2)
        .filter(|m| m.is_writable)
        .map(|m| m.pubkey)
        .collect();
    let mut expected = expected.to_vec();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected, "wrong write set");
    let unchanged: Vec<_> = ix
        .accounts
        .iter()
        .skip(2)
        .filter(|m| !m.is_writable)
        .map(|m| (m.pubkey, h.svm.get_account(&m.pubkey)))
        .collect();
    // Every declared Ledger write is necessary. A missing permission must undo
    // earlier allocations and writes, not merely fail for an unrelated reason.
    for key in &expected {
        let mut missing = ix.clone();
        readonly(&mut missing, *key);
        rejects(h, &[0], missing, LedgerError::InvalidAccount.into());
    }
    succeeds(h, &[0], ix);
    for (key, before) in unchanged {
        assert_eq!(h.svm.get_account(&key), before, "read-only account changed");
    }
}

fn add_group(
    h: &mut Harness,
    root: Address,
    parent: Address,
    tag: u8,
    extra: &[Address],
) -> Address {
    let relative = Address::new_from_array([tag; 32]);
    let ix = group(h, root, h.key(0), parent, relative, true, extra);
    succeeds(h, &[0], ix);
    child(parent, relative)
}

#[test]
fn debit_paths_stop_below_common_ancestor_at_equal_and_unequal_depths() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 170, 0);
    let app = branch(&mut h, e.root, 0, true);
    let left = add_group(&mut h, e.root, app, 172, &[app]);
    let deep = add_group(&mut h, e.root, left, 173, &[app]);
    let right = add_group(&mut h, e.root, app, 174, &[app]);
    let a = h.key(1);
    let b = h.key(2);
    let deposit = e.movement(&h, (h.key(0), 0), (deep, a), 100, true, &[left, app]);
    succeeds(&mut h, &[0], deposit);

    let ix = planned(
        &h,
        transfer(
            &h,
            e.root,
            h.key(0),
            (deep, a),
            (right, b),
            30,
            &[left, app],
        ),
    );
    check_plan(
        &mut h,
        ix,
        &[child(deep, a), deep, left, child(right, b), right],
    );
    assert_eq!(h.record(left).debit, 70);
    assert_eq!(h.record(right).debit, 30);
    assert_eq!(h.record(app).debit, 100);
    assert_eq!(h.record(e.root).debit, 100);
    assert!(!h.record(child(right, b)).registered);

    let ix = planned(
        &h,
        transfer(&h, e.root, h.key(0), (deep, a), (deep, b), 10, &[left, app]),
    );
    check_plan(&mut h, ix, &[child(deep, a), child(deep, b)]);
    assert_eq!(h.record(deep).debit, 70);
    assert_eq!(h.record(child(deep, a)).debit, 60);
    assert_eq!(h.record(child(deep, b)).debit, 10);

    // Events for zero amounts and self-transfer checks need no data writes.
    for (to, amount) in [((deep, b), 0), ((deep, a), 20)] {
        let ix = planned(
            &h,
            transfer(&h, e.root, h.key(0), (deep, a), to, amount, &[left, app]),
        );
        check_plan(&mut h, ix, &[]);
    }
}

#[test]
fn credit_paths_use_effective_leaf_polarity_and_leave_app_and_ledger_readonly() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let app = add_group(&mut h, root, root, 180, &[]);
    let relative = Address::new_from_array([181; 32]);
    let credit_group = child(app, relative);
    let ix = ix(
        h.base(0, root),
        instruction::AddSubAccountGroup {
            parent: ap(app),
            relative: ap(relative),
            name: "Credit".into(),
            credit: true,
            implicit_allowed: true,
        },
        &[app, credit_group],
    );
    succeeds(&mut h, &[0], ix);
    let a = h.key(1);
    let b = h.key(2);
    // Implicit credit inherits its credit parent. The app above it is a debit group.
    let mint = planned(
        &h,
        transfer(&h, root, h.key(0), (credit_group, a), (app, a), 100, &[app]),
    );
    check_plan(
        &mut h,
        mint,
        &[
            child(credit_group, a),
            credit_group,
            child(app, a),
            app,
            root,
        ],
    );
    // Explicit credit overrides its debit parent's polarity.
    let register = leaf(&h, root, h.key(0), app, b, "Credit leaf", true);
    succeeds(&mut h, &[0], register);
    let ix = planned(
        &h,
        transfer(&h, root, h.key(0), (app, b), (credit_group, a), 25, &[app]),
    );
    check_plan(
        &mut h,
        ix,
        &[child(app, b), child(credit_group, a), credit_group],
    );
    assert_eq!(h.record(child(app, b)).credit, 25);
    assert_eq!(h.record(child(credit_group, a)).credit, 75);
    assert_eq!(h.record(app).credit, 100);
    assert_eq!(h.record(root).credit, 100);
    // Burn below the app still changes both columns all the way to the token root.
    let burn = planned(
        &h,
        transfer(&h, root, h.key(0), (app, a), (app, b), 10, &[]),
    );
    check_plan(&mut h, burn, &[child(app, a), child(app, b), app, root]);
    assert_eq!((h.record(root).debit, h.record(root).credit), (90, 90));
}

#[test]
fn cpi_preserves_readonly_common_ancestors() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 190, 0);
    let program = Address::new_from_array([192; 32]);
    let binary = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
    )
    .unwrap();
    h.svm.add_program(program, &binary).unwrap();
    let authority = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"app", h.key(0).as_ref()],
        &ap(program),
    )
    .0);
    let app = child(e.root, authority);
    let create = proxy(
        &h,
        program,
        authority,
        group(&h, e.root, authority, e.root, authority, true, &[]),
    );
    succeeds(&mut h, &[0], create);
    let a = h.key(1);
    let b = h.key(2);
    let deposit = proxy(
        &h,
        program,
        authority,
        e.movement(&h, (authority, 0), (app, a), 100, true, &[]),
    );
    succeeds(&mut h, &[0], deposit);
    let before = [e.root, app].map(|key| h.svm.get_account(&key));
    let inner = planned(
        &h,
        transfer(&h, e.root, authority, (app, a), (app, b), 20, &[]),
    );
    for key in [e.root, app] {
        assert!(
            !inner
                .accounts
                .iter()
                .find(|m| m.pubkey == key)
                .unwrap()
                .is_writable
        );
    }
    let outer = proxy(&h, program, authority, inner);
    succeeds(&mut h, &[0], outer);
    assert_eq!([e.root, app].map(|key| h.svm.get_account(&key)), before);
    assert_eq!(h.record(child(app, b)).debit, 20);
}

#[test]
fn tree_mutations_require_only_changed_ancestors_and_repeats_need_no_root_write() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let app = add_group(&mut h, root, root, 200, &[]);
    let root_before = h.svm.get_account(&root);
    let relative = h.key(1);
    let mut create = leaf(&h, root, h.key(0), app, relative, "Leaf", false);
    readonly(&mut create, root);
    succeeds(&mut h, &[0], create.clone());
    assert_eq!(h.svm.get_account(&root), root_before);
    // A matching repeat has no record writes or index changes.
    readonly(&mut create, app);
    readonly(&mut create, child(app, relative));
    succeeds(&mut h, &[0], create);
    let mut remove = remove(&h, root, h.key(0), app, relative, false);
    readonly(&mut remove, root);
    succeeds(&mut h, &[0], remove);
    assert_eq!(h.svm.get_account(&root), root_before);
    let mut direct = leaf(&h, root, h.key(0), root, relative, "Direct", false);
    readonly(&mut direct, root);
    rejects(&mut h, &[0], direct, LedgerError::InvalidAccount.into());
}
