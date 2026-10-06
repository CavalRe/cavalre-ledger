//! A custody shortfall freezes mutations, not reads or uncredited repairs.
use super::*;

fn set_amount(h: &mut Harness, vault: Address, amount: u64) {
    // Simulate a custody loss; ordinary users cannot edit token-owned accounts.
    let mut account = h.svm.get_account(&vault).unwrap();
    let mut token = TokenAccount::unpack(&account.data[..TokenAccount::LEN]).unwrap();
    token.amount = amount;
    TokenAccount::pack(token, &mut account.data[..TokenAccount::LEN]).unwrap();
    h.svm.set_account(vault, account).unwrap();
}

fn top_up(h: &mut Harness, e: &External, amount: u64) {
    let instruction = spl_token_2022_interface::instruction::transfer_checked(
        &ap(e.token_program),
        &ap(e.wallet),
        &ap(e.mint),
        &ap(e.vault),
        &ap(h.key(1)),
        &[],
        amount,
        6,
    )
    .unwrap();
    succeeds(h, &[0, 1], instruction);
}

#[test]
fn token_shortfalls_freeze_all_mutations_direct_and_cpi_and_recover_without_admin() {
    for token2022 in [false, true] {
        for cpi in [false, true] {
            let mut h = Harness::new();
            let e = if token2022 {
                let e = super::token2022::initialized(&mut h, true);
                let registration = e.registration(&h);
                succeeds(&mut h, &[0], registration);
                e
            } else {
                External::new(&mut h, 60, 1)
            };
            let app = Address::new_from_array([62; 32]);
            let authority = if cpi {
                h.svm
                    .add_program(
                        app,
                        &std::fs::read(
                            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                                .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                sa(anchor_lang::prelude::Pubkey::find_program_address(
                    &[b"app", h.key(0).as_ref()],
                    &ap(app),
                )
                .0)
            } else {
                h.key(0)
            };
            let call = |h: &Harness, i| if cpi { proxy(h, app, authority, i) } else { i };
            let parent = child(e.root, authority);
            let create = group(&h, e.root, authority, e.root, authority, true, &[]);
            let create = call(&h, create);
            succeeds(&mut h, &[0], create);
            let user = h.key(2);
            let empty = Address::new_from_array([64; 32]);
            let empty_group = Address::new_from_array([65; 32]);
            let fresh = Address::new_from_array([66; 32]);
            for create in [
                leaf(&h, e.root, authority, parent, empty, "Empty", false),
                group(&h, e.root, authority, parent, empty_group, true, &[]),
            ] {
                let create = call(&h, create);
                succeeds(&mut h, &[0], create);
            }
            let deposit = call(
                &h,
                e.movement(&h, (authority, 1), (parent, user), 100, true, &[]),
            );
            succeeds(&mut h, &[0, 1], deposit);
            let mutations = |h: &Harness| {
                let mut instructions = vec![
                    group(h, e.root, authority, e.root, authority, true, &[]), // repeat/no-op
                    leaf(h, e.root, authority, parent, fresh, "New", false),
                    group(h, e.root, authority, parent, fresh, true, &[]),
                    remove(h, e.root, authority, parent, empty, false),
                    remove(h, e.root, authority, parent, empty_group, true),
                    remove(h, e.root, authority, parent, fresh, false), // absent/no-op
                    transfer(
                        h,
                        e.root,
                        authority,
                        (parent, user),
                        (parent, fresh),
                        1,
                        &[],
                    ),
                    transfer(
                        h,
                        e.root,
                        authority,
                        (parent, user),
                        (parent, fresh),
                        0,
                        &[],
                    ),
                    transfer(h, e.root, authority, (parent, user), (parent, user), 1, &[]),
                ];
                for group in [false, true] {
                    let name = "Named".to_string();
                    let relative = sa(ledger::ledger_lib::name_to_address(&name).unwrap());
                    let data = if group {
                        instruction::AddSubAccountGroupByName {
                            parent: ap(parent),
                            name,
                            credit: false,
                        }
                        .data()
                    } else {
                        instruction::AddSubAccountByName {
                            parent: ap(parent),
                            name,
                            credit: false,
                        }
                        .data()
                    };
                    let mut i = leaf(h, e.root, authority, parent, relative, "Named", false);
                    i.data = data;
                    instructions.push(i);
                }
                let mut calls = instructions
                    .into_iter()
                    .map(|i| (vec![0], call(h, i)))
                    .collect::<Vec<_>>();
                for deposit in [false, true] {
                    for amount in [0, 1] {
                        calls.push((
                            vec![0, 1],
                            call(
                                h,
                                e.movement(h, (authority, 1), (parent, user), amount, deposit, &[]),
                            ),
                        ));
                    }
                }
                calls.push((vec![0], e.registration(h)));
                calls
            };
            let records = [e.root_storage, e.source, parent, child(parent, user)]
                .map(|key| (key, h.svm.get_account(&key).unwrap()));
            set_amount(&mut h, e.vault, 80);
            for shortfall in [20, 1] {
                assert_eq!(h.token(e.vault), 100 - shortfall);
                for (signers, mutation) in mutations(&h) {
                    rejects(
                        &mut h,
                        &signers,
                        mutation,
                        LedgerError::Undercollateralized.into(),
                    );
                }
                // Reads consume only Ledger records; no vault or signer needed.
                let mut reader = ledger::ledger_view::Reader::new();
                for (key, _) in &records {
                    let account = h.svm.get_account(key).unwrap();
                    reader
                        .insert(ap(*key), &ap(account.owner), &account.data)
                        .unwrap();
                }
                assert_eq!(reader.total_supply(&ap(e.root)).unwrap(), 100);
                assert_eq!(
                    reader
                        .account_view(&ap(e.root), &ap(parent), &ap(user))
                        .unwrap()
                        .balances
                        .debit,
                    100
                );
                if shortfall == 20 {
                    top_up(&mut h, &e, 19);
                }
            }
            // An unrelated token ledger and an accounting-only ledger still work.
            let other = External::new(&mut h, 90, 0);
            branch(&mut h, other.root, 0, true);
            h.internal();
            top_up(&mut h, &e, 1);
            for (key, old) in records {
                assert_eq!(h.svm.get_account(&key).unwrap(), old);
            }
            let transfer = call(
                &h,
                transfer(
                    &h,
                    e.root,
                    authority,
                    (parent, user),
                    (parent, fresh),
                    1,
                    &[],
                ),
            );
            succeeds(&mut h, &[0], transfer);
            for deposit in [true, false] {
                let movement = call(
                    &h,
                    e.movement(&h, (authority, 1), (parent, user), 1, deposit, &[]),
                );
                succeeds(&mut h, &[0, 1], movement);
            }
            let create = call(&h, leaf(&h, e.root, authority, parent, fresh, "New", false));
            succeeds(&mut h, &[0], create);
            assert_eq!(h.token(e.vault), 100);
            assert_eq!(h.record(e.root).credit, 100);
        }
    }
}

#[test]
fn ordinary_mutations_require_an_authentic_canonical_vault_readonly() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let parent = branch(&mut h, e.root, 0, true);
    let mutation = transfer(
        &h,
        e.root,
        h.key(0),
        (parent, h.key(1)),
        (parent, h.key(2)),
        0,
        &[],
    );
    let complete = h.indexed(mutation.clone());
    assert!(
        !complete
            .accounts
            .iter()
            .find(|a| a.pubkey == e.vault)
            .unwrap()
            .is_writable
    );
    let failure = run_raw(&mut h, &[0], mutation.clone()).unwrap_err();
    assert_eq!(
        failure.err,
        TransactionError::InstructionError(
            0,
            InstructionError::Custom(LedgerError::MissingAccount.into())
        )
    );
    let forged_key = Address::new_from_array([75; 32]);
    let original = h.svm.get_account(&e.vault).unwrap();
    h.svm.set_account(forged_key, original.clone()).unwrap();
    let mut substituted = mutation.clone();
    substituted
        .accounts
        .push(AccountMeta::new_readonly(forged_key, false));
    let failure = run_raw(&mut h, &[0], substituted).unwrap_err();
    assert_eq!(
        failure.err,
        TransactionError::InstructionError(
            0,
            InstructionError::Custom(LedgerError::MissingAccount.into())
        )
    );
    for fault in 0..3 {
        let mut forged = original.clone();
        if fault == 0 {
            forged.owner = SYSTEM;
        } else {
            let mut state = TokenAccount::unpack(&forged.data).unwrap();
            if fault == 1 {
                state.mint = forged_key;
            } else {
                state.owner = forged_key;
            }
            TokenAccount::pack(state, &mut forged.data).unwrap();
        }
        h.svm.set_account(e.vault, forged).unwrap();
        rejects(
            &mut h,
            &[0],
            mutation.clone(),
            LedgerError::InvalidAccount.into(),
        );
    }
    let mut malformed = original.clone();
    malformed.data.clear();
    h.svm.set_account(e.vault, malformed).unwrap();
    assert!(run_raw(&mut h, &[0], complete.clone()).is_err());
    h.svm.set_account(e.vault, original).unwrap();
    run_raw(&mut h, &[0], complete).unwrap();
}

#[test]
fn custody_binding_is_immutable_and_missing_bindings_fail_closed() {
    for token2022 in [false, true] {
        let mut h = Harness::new();
        let e = if token2022 {
            let e = super::token2022::initialized(&mut h, true);
            let registration = e.registration(&h);
            succeeds(&mut h, &[0], registration);
            e
        } else {
            External::new(&mut h, 60, 0)
        };
        let parent = branch(&mut h, e.root, 0, true);
        let root = h.svm.get_account(&e.root_storage).unwrap();
        assert_eq!(
            root.data.len(),
            ledger::ledger_storage::LEDGER_HEADER_LEN + 4 + 32 * 2
        );
        assert_eq!(
            &root.data[ledger::ledger_storage::VAULT..ledger::ledger_storage::VAULT + 32],
            e.vault.as_ref()
        );
        let mutation = transfer(
            &h,
            e.root,
            h.key(0),
            (parent, h.key(1)),
            (parent, h.key(2)),
            0,
            &[],
        );
        for replacement in [SYSTEM, Address::new_from_array([76; 32])] {
            // Corruption fixture, not a user-writable setting. Registration may
            // not repair/replace this trusted identity from instruction input.
            let mut corrupted = root.clone();
            corrupted.data[ledger::ledger_storage::VAULT..ledger::ledger_storage::VAULT + 32]
                .copy_from_slice(replacement.as_ref());
            h.svm.set_account(e.root_storage, corrupted).unwrap();
            rejects(
                &mut h,
                &[0],
                mutation.clone(),
                LedgerError::InvalidAccount.into(),
            );
            let registration = e.registration(&h);
            rejects(
                &mut h,
                &[0],
                registration,
                LedgerError::InvalidAccount.into(),
            );
        }
        h.svm.set_account(e.root_storage, root).unwrap();
        let registration = e.registration(&h);
        let before = h.svm.get_account(&e.root_storage);
        succeeds(&mut h, &[0], registration);
        succeeds(&mut h, &[0], mutation);
        assert_eq!(h.svm.get_account(&e.root_storage), before);
    }
}
