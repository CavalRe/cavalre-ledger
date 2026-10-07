//! Recipient creation and the sole transfer instruction, measured independently.
use super::*;
use ledger::ledger_lib::Child;

fn create(
    h: &Harness,
    root: Address,
    parent: Address,
    relative: Address,
    rest: &[Address],
) -> Instruction {
    let remaining: Vec<_> = rest
        .iter()
        .map(|key| {
            anchor_lang::solana_program::instruction::AccountMeta::new_readonly(ap(*key), false)
        })
        .collect();
    let call = ledger::ledger_transfer::create_idempotent_instruction(
        ap(h.key(0)),
        ap(h.key(0)),
        ap(root),
        Child {
            parent: ap(parent),
            relative: ap(relative),
        },
        &remaining,
    );
    Instruction {
        program_id: sa(call.program_id),
        data: call.data,
        accounts: call
            .accounts
            .into_iter()
            .map(|m| AccountMeta {
                pubkey: sa(m.pubkey),
                is_writable: m.is_writable,
                is_signer: m.is_signer,
            })
            .collect(),
    }
}

#[test]
fn create_idempotent_preserves_custody_and_measures_first_receipt() {
    for token_program in [TOKEN, TOKEN_2022] {
        let mut h = Harness::new();
        let e = External::setup(&mut h, 130, 0, token_program);
        h.metadata(e.mint, "Token", "TOK");
        let registration = e.registration(&h);
        succeeds(&mut h, &[0], registration);
        let alice = h.key(0);
        let bob = h.key(1);
        let deposit = e.movement(&h, (alice, 0), (e.root, alice), 100, true, &[]);
        succeeds(&mut h, &[0], deposit);
        let bob_account = child(e.root, bob);
        let alice_account = child(e.root, alice);
        let creation = create(&h, e.root, e.root, bob, &[e.vault]);
        let send = packed_transfers::fast(
            &h,
            e.root,
            alice,
            (e.root, alice),
            (e.root, bob),
            7,
            &[e.vault],
        );
        let keys = [e.root, e.source, e.vault, alice_account, bob_account];
        let before = keys.map(|key| h.svm.get_account(&key));

        // Transfer itself cannot create anything, even though the recipient PDA
        // is supplied. Reject the removed Anchor Transfer discriminator too.
        assert!(run_raw(&mut h, &[0], send.clone()).is_err());
        let mut removed = send.clone();
        // The actual former Anchor ABI used sha256("global:transfer")[..8].
        removed.data[..8].copy_from_slice(&[163, 52, 200, 231, 140, 3, 69, 186]);
        removed.data.truncate(152);
        assert!(run_raw(&mut h, &[0], removed).is_err());
        assert_eq!(keys.map(|key| h.svm.get_account(&key)), before);

        // A later transfer failure rolls back successful creation and all rent.
        let payer_before = h.svm.get_account(&alice).unwrap().lamports;
        let mut overdraft = send.clone();
        overdraft.data[136..152].copy_from_slice(&101u128.to_le_bytes());
        let failed = run_instructions(&mut h, &[0], &[creation.clone(), overdraft]).unwrap_err();
        assert!(matches!(
            failed.err,
            TransactionError::InstructionError(1, _)
        ));
        assert_eq!(keys.map(|key| h.svm.get_account(&key)), before);
        assert_eq!(
            h.svm.get_account(&alice).unwrap().lamports + failed.meta.fee,
            payer_before
        );

        let payer_before = h.svm.get_account(&alice).unwrap().lamports;
        let first = run_instructions(&mut h, &[0], &[creation.clone(), send.clone()]).unwrap();
        let rent = payer_before - h.svm.get_account(&alice).unwrap().lamports - first.fee;
        let bob_record = h.record(bob_account);
        assert_eq!(bob_record.parent, ap(e.root));
        assert_eq!(bob_record.custodian, ap(bob_account));
        assert_eq!(
            (bob_record.kind, bob_record.debit, bob_record.credit),
            (2, 7, 0)
        );
        assert_eq!(
            h.record(e.root)
                .children
                .iter()
                .filter(|r| **r == ap(bob))
                .count(),
            1
        );
        assert_eq!(h.record(alice_account).debit, 93);
        assert_eq!(events::event_bytes(&first.logs).len(), 2);
        let after = keys.map(|key| h.svm.get_account(&key));

        // Repeating creation accepts read-only records, takes no rent, preserves
        // balances/indexes, and emits no duplicate lifecycle/posting events.
        let mut repeat = creation.clone();
        for meta in repeat.accounts.iter_mut().skip(2) {
            meta.is_writable = false;
        }
        let payer_before = h.svm.get_account(&alice).unwrap().lamports;
        let repeated = run_raw(&mut h, &[0], repeat).unwrap();
        assert!(
            repeated.compute_units_consumed <= 1_300,
            "idempotent creation CU regression"
        );
        assert_eq!(keys.map(|key| h.svm.get_account(&key)), after);
        assert_eq!(
            h.svm.get_account(&alice).unwrap().lamports + repeated.fee,
            payer_before
        );
        assert!(events::event_bytes(&repeated.logs).is_empty());
        // The shortcut still authenticates identities, signers, the ledger and
        // live backing. Bad inputs cannot turn an existing leaf into a no-op.
        for fault in 0..6 {
            let mut bad = creation.clone();
            match fault {
                0 => bad.data[72] ^= 1,
                1 => {
                    bad.data.pop();
                }
                2 => {
                    bad.accounts[1].pubkey = bob;
                    bad.accounts[1].is_signer = false;
                }
                3 => bad.accounts[2].pubkey = e.source,
                4 => bad.accounts[3].pubkey = bob,
                _ => bad.accounts.retain(|m| m.pubkey != e.vault),
            }
            assert!(run_raw(&mut h, &[0], bad).is_err());
            assert_eq!(keys.map(|key| h.svm.get_account(&key)), after);
        }
        let original_vault = h.svm.get_account(&e.vault).unwrap();
        let mut short = original_vault.clone();
        short.data[64..72].copy_from_slice(&0u64.to_le_bytes());
        h.svm.set_account(e.vault, short).unwrap();
        let failure = run_raw(&mut h, &[0], creation.clone()).unwrap_err();
        assert_eq!(
            failure.err,
            TransactionError::InstructionError(
                0,
                InstructionError::Custom(LedgerError::Undercollateralized.into())
            )
        );
        h.svm.set_account(e.vault, original_vault).unwrap();
        let existing_pair =
            run_instructions(&mut h, &[0], &[creation.clone(), send.clone()]).unwrap();
        assert!(existing_pair.compute_units_consumed <= 3_500);
        let existing = run_raw(&mut h, &[0], send.clone()).unwrap();
        assert!(existing.compute_units_consumed <= 2_150);

        // Funding Bob's storage grants Alice no right to debit Bob's balance.
        let steal = packed_transfers::fast(
            &h,
            e.root,
            alice,
            (e.root, bob),
            (e.root, alice),
            1,
            &[e.vault],
        );
        assert!(run_raw(&mut h, &[0], steal).is_err());
        let spend = packed_transfers::fast(
            &h,
            e.root,
            bob,
            (e.root, bob),
            (e.root, alice),
            1,
            &[e.vault],
        );
        run_raw(&mut h, &[0, 1], spend).unwrap();

        // Return to the same pre-state to measure creation alone fairly.
        for (key, account) in keys.into_iter().zip(before) {
            h.svm.set_account(key, account.unwrap_or_default()).unwrap();
        }
        let created = run_raw(&mut h, &[0], creation).unwrap();
        eprintln!("RECIPIENT_CU token={token_program} create_and_transfer={} create={} create_existing={} existing_pair={} transfer={} storage_lamports={rent}",
            first.compute_units_consumed, created.compute_units_consumed,
            repeated.compute_units_consumed, existing_pair.compute_units_consumed, existing.compute_units_consumed);
    }
}

#[test]
fn create_idempotent_rejects_wrong_identity_groups_and_internal_authority() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 133, 0);
    let app = branch(&mut h, e.root, 0, true);
    let relative = h.key(1);
    let account = child(app, relative);
    let valid = create(&h, e.root, app, relative, &[e.vault]);
    let before = [e.root, app, account].map(|key| h.svm.get_account(&key));
    let mut wrong = valid.clone();
    wrong
        .accounts
        .iter_mut()
        .find(|m| m.pubkey == account)
        .unwrap()
        .pubkey = h.key(2);
    assert!(run_raw(&mut h, &[0], wrong).is_err());
    let mut readonly = valid.clone();
    readonly
        .accounts
        .iter_mut()
        .find(|m| m.pubkey == app)
        .unwrap()
        .is_writable = false;
    assert!(run_raw(&mut h, &[0], readonly).is_err());
    assert_eq!(
        [e.root, app, account].map(|key| h.svm.get_account(&key)),
        before
    );
    let group = create(&h, e.root, e.root, h.key(0), &[e.vault]);
    assert!(run_raw(&mut h, &[0], group).is_err());
    let source = create(&h, e.root, e.root, sa(SOURCE), &[e.vault]);
    assert!(run_raw(&mut h, &[0], source).is_err());
    // A prefunded System-owned PDA is safely completed rather than stranded.
    h.svm.airdrop(&account, 1_000_000).unwrap();
    run_raw(&mut h, &[0], valid).unwrap();
    assert_eq!(h.record(account).custodian, ap(app));
    assert_eq!(h.record(account).debit, 0);

    let (root, _) = h.internal();
    let valid = create(&h, root, root, relative, &[]);
    let mut bad = valid.clone();
    bad.accounts[1].pubkey = h.key(1);
    assert!(run_raw(&mut h, &[0, 1], bad.clone()).is_err());
    assert!(h.svm.get_account(&child(root, relative)).is_none());
    run_raw(&mut h, &[0], valid.clone()).unwrap();
    // Existing internal leaves retain the same authority requirement.
    assert!(run_raw(&mut h, &[0, 1], bad).is_err());
    run_raw(&mut h, &[0], valid).unwrap();
}
