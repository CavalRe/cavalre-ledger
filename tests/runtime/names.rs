//! Named creation must retain explicit creation's identity and permissions.
use super::*;
use ledger::ledger_lib::{name_to_address, to_address_by_name, AccountKind};

fn named(
    h: &Harness,
    root: Address,
    authority: Address,
    parent: Address,
    name: &str,
    kind: AccountKind,
) -> Instruction {
    let relative = name_to_address(name).map(sa).unwrap_or_default();
    let rest = remaining(root, &[parent, child(parent, relative)]);
    if kind.is_group() {
        ix(
            base(h, root, authority),
            instruction::AddSubAccountGroupByName {
                parent: ap(parent),
                name: name.into(),
                credit: kind.is_credit(),
                implicit_allowed: true,
            },
            &rest,
        )
    } else {
        ix(
            base(h, root, authority),
            instruction::AddSubAccountByName {
                parent: ap(parent),
                name: name.into(),
                credit: kind.is_credit(),
            },
            &rest,
        )
    }
}

#[test]
fn named_and_explicit_creation_share_records_events_and_idempotence() {
    for kind in [
        AccountKind::DebitGroup,
        AccountKind::CreditGroup,
        AccountKind::DebitLedger,
        AccountKind::CreditLedger,
    ] {
        let mut a = Harness::new();
        let (root, _) = a.internal();
        let mut b = Harness::new();
        b.internal();
        let name = "é".repeat(32); // Maximum length is bytes, not characters.
        let relative = sa(name_to_address(&name).unwrap());
        let absolute = child(root, relative);
        assert_eq!(
            absolute,
            sa(to_address_by_name(&ledger::ID, &ap(root), &name).unwrap().0)
        );
        let by_name = named(&a, root, a.key(0), root, &name, kind);
        let explicit = if kind.is_group() {
            ix(
                base(&b, root, b.key(0)),
                instruction::AddSubAccountGroup {
                    parent: ap(root),
                    relative: ap(relative),
                    name: name.clone(),
                    credit: kind.is_credit(),
                    implicit_allowed: true,
                },
                &[absolute],
            )
        } else {
            leaf(&b, root, b.key(0), root, relative, &name, kind.is_credit())
        };
        let result_a = run(&mut a, &[0], by_name.clone()).unwrap();
        let result_b = run(&mut b, &[0], explicit.clone()).unwrap();
        assert_eq!(
            events::event_bytes(&result_a.logs),
            events::event_bytes(&result_b.logs)
        );
        let slot = sa(ledger::ledger_lib::child_index_address(&ap(root), 1).0);
        for key in [root, absolute, slot] {
            assert_eq!(a.svm.get_account(&key), b.svm.get_account(&key));
        }
        assert_eq!(a.svm.get_account(&a.key(0)), b.svm.get_account(&b.key(0)));
        // Either form can repeat the other's registration with no Ledger writes,
        // events or rent. No slot input is required for matching registration.
        let before = [root, absolute, slot].map(|k| a.svm.get_account(&k));
        for mut repeat in [by_name.clone(), explicit] {
            for meta in repeat.accounts.iter_mut().skip(2) {
                meta.is_writable = false;
            }
            let lamports = a.svm.get_account(&a.key(0)).unwrap().lamports;
            let result = run_raw(&mut a, &[0], repeat).unwrap();
            assert!(events::event_bytes(&result.logs).is_empty());
            assert_eq!(
                a.svm.get_account(&a.key(0)).unwrap().lamports + result.fee,
                lamports
            );
            assert_eq!(
                [root, absolute, slot].map(|k| a.svm.get_account(&k)),
                before
            );
        }
        let opposite = if kind.is_group() {
            if kind.is_credit() {
                AccountKind::DebitGroup
            } else {
                AccountKind::CreditGroup
            }
        } else if kind.is_credit() {
            AccountKind::DebitLedger
        } else {
            AccountKind::CreditLedger
        };
        let conflict = named(&a, root, a.key(0), root, &name, opposite);
        rejects(&mut a, &[0], conflict, LedgerError::MetadataConflict.into());
        let unauthorized = named(&a, root, a.key(1), root, &name, kind);
        rejects(
            &mut a,
            &[0, 1],
            unauthorized,
            LedgerError::Unauthorized.into(),
        );
        eprintln!(
            "named {kind:?}: {} CUs; explicit: {} CUs",
            result_a.compute_units_consumed, result_b.compute_units_consumed
        );
    }
}

#[test]
fn named_leaves_preserve_implicit_balances_and_parent_scoping() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let parent = branch(&mut h, root, 0, true);
    let relative = sa(name_to_address("Rewards").unwrap());
    let absolute = child(parent, relative);
    let fund = transfer(
        &h,
        root,
        h.key(0),
        (root, sa(SOURCE)),
        (parent, relative),
        17,
        &[],
    );
    succeeds(&mut h, &[0], fund);
    assert!(!h.record(absolute).registered);
    let register = named(
        &h,
        root,
        h.key(0),
        parent,
        "Rewards",
        AccountKind::DebitLedger,
    );
    succeeds(&mut h, &[0], register);
    assert!(h.record(absolute).registered);
    assert_eq!(h.record(absolute).debit, 17);
    let register_elsewhere = named(
        &h,
        root,
        h.key(0),
        root,
        "Rewards",
        AccountKind::DebitLedger,
    );
    succeeds(&mut h, &[0], register_elsewhere);
    assert_eq!(h.record(child(root, relative)).debit, 0);
    assert_ne!(child(root, relative), absolute);
    for name in [String::new(), "x".repeat(65), "é".repeat(33)] {
        for kind in [AccountKind::DebitGroup, AccountKind::DebitLedger] {
            let invalid = named(&h, root, h.key(0), parent, &name, kind);
            rejects(&mut h, &[0], invalid, LedgerError::InvalidName.into());
        }
    }
    // A display name does not reserve the name-derived identity.
    let another = leaf(&h, root, h.key(0), parent, h.key(2), "Rewards", false);
    succeeds(&mut h, &[0], another);
    assert_eq!(h.record(absolute).debit, 17);
}

#[test]
fn named_creation_preserves_external_custody_and_cpi_signer_rules() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 230, 0);
    // A name-derived identifier gives the caller no authority at a token root.
    let direct = named(
        &h,
        e.root,
        h.key(0),
        e.root,
        "Rewards",
        AccountKind::DebitGroup,
    );
    rejects(&mut h, &[0], direct, LedgerError::Unauthorized.into());
    let app = Address::new_from_array([232; 32]);
    let binary = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
    )
    .unwrap();
    h.svm.add_program(app, &binary).unwrap();
    let authority = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"app", h.key(0).as_ref()],
        &ap(app),
    )
    .0);
    let parent = child(e.root, authority);
    let create = proxy(
        &h,
        app,
        authority,
        group(&h, e.root, authority, e.root, authority, true, &[]),
    );
    succeeds(&mut h, &[0], create);
    let register = named(
        &h,
        e.root,
        authority,
        parent,
        "Rewards",
        AccountKind::DebitGroup,
    );
    let create = proxy(&h, app, authority, register.clone());
    succeeds(&mut h, &[0], create);
    let rewards = child(parent, sa(name_to_address("Rewards").unwrap()));
    assert_eq!(h.record(rewards).custodian, ap(parent));
    let administrator = named(
        &h,
        e.root,
        h.key(0),
        parent,
        "Rewards",
        AccountKind::DebitGroup,
    );
    rejects(
        &mut h,
        &[0],
        administrator,
        LedgerError::Unauthorized.into(),
    );
    let credit = proxy(
        &h,
        app,
        authority,
        named(
            &h,
            e.root,
            authority,
            parent,
            "Payable",
            AccountKind::CreditLedger,
        ),
    );
    rejects(&mut h, &[0], credit, LedgerError::InvalidKind.into());
    // Nested named leaves retain the original application's custody identity.
    let mut nested = named(
        &h,
        e.root,
        authority,
        rewards,
        "Available",
        AccountKind::DebitLedger,
    );
    nested
        .accounts
        .push(AccountMeta::new_readonly(parent, false));
    let create = proxy(&h, app, authority, nested);
    succeeds(&mut h, &[0], create);
    assert_eq!(
        h.record(child(rewards, sa(name_to_address("Available").unwrap())))
            .custodian,
        ap(parent)
    );
    let repeat = proxy(&h, app, authority, register);
    let result = run(&mut h, &[0], repeat).unwrap();
    assert!(events::event_bytes(&result.logs).is_empty());
}
