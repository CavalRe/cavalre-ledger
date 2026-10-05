//! Ledger identity is independent of the Solana account holding its record.
use super::*;
use ledger::ledger_lib::{
    child_index_address, decode_child_data, decode_data, global_root_address,
};
use ledger::ledger_view::Reader;

#[test]
fn mint_is_the_ledger_identity_in_storage_discovery_parents_reads_and_posting() {
    for program in [TOKEN, TOKEN_2022] {
        let mut h = Harness::new();
        let e = External::setup(&mut h, 170, 1, program);
        let mint_before = h.svm.get_account(&e.mint).unwrap();
        assert_eq!(e.root, e.mint);
        assert_ne!(e.root_storage, e.mint);
        let registration = e.registration(&h);
        succeeds(&mut h, &[0], registration.clone());
        let repeated = run(&mut h, &[0], registration).unwrap();
        assert!(events::event_bytes(&repeated.logs).is_empty());
        let root = h.record(e.root);
        assert_eq!(
            (root.root, root.relative, root.identifier),
            (ap(e.mint), ap(e.mint), ap(e.mint))
        );
        let global = global_root_address().0;
        let slot_key = child_index_address(&global, 0).0;
        let slot = h.svm.get_account(&sa(slot_key)).unwrap();
        assert_eq!(
            decode_child_data(&slot_key, &ap(slot.owner), &slot.data)
                .unwrap()
                .relative,
            Some(ap(e.mint))
        );

        let app = branch(&mut h, e.root, 0, true);
        assert_eq!(h.record(app).parent, ap(e.mint));
        assert_eq!(h.record(e.source).parent, ap(e.mint));
        // The leaf identifier is a name hash with no Solana account or signer.
        let relative = sa(ledger::ledger_lib::name_to_address("Arbitrary identity").unwrap());
        assert!(h.svm.get_account(&relative).is_none());
        let create = leaf(&h, e.root, h.key(0), app, relative, "", false);
        succeeds(&mut h, &[0], create);
        let deposit = e.movement(&h, (h.key(0), 1), (app, relative), 10, true, &[]);
        succeeds(&mut h, &[0, 1], deposit);
        let position = child(app, relative);
        assert_eq!(h.record(position).root, ap(e.mint));
        assert_eq!(h.svm.get_account(&e.mint).unwrap(), mint_before);
        assert!(h.svm.get_account(&relative).is_none());

        // Both the token mint and its separate Ledger record can be supplied,
        // in either order. All view arguments and returned identities use mint.
        for reverse in [false, true] {
            let mut keys = vec![e.root_storage, e.mint, app, position, e.source];
            if reverse {
                keys.reverse();
            }
            let mut reader = Reader::new();
            for key in keys {
                let account = h.svm.get_account(&key).unwrap();
                reader
                    .insert(ap(key), &ap(account.owner), &account.data)
                    .unwrap();
            }
            assert_eq!(reader.total_supply(&ap(e.mint)).unwrap(), 10);
            assert_eq!(reader.ledger(&ap(app)).unwrap(), Some(ap(e.mint)));
            assert_eq!(reader.known_ledgers().unwrap(), vec![ap(e.mint)]);
            assert_eq!(
                reader.total_supply(&ap(e.root_storage)),
                Err(ledger::ledger_lib::Error::MissingAccount)
            );
            let writable = reader
                .transfer_writable_accounts(
                    &ap(e.mint),
                    ledger::ledger_lib::Child {
                        parent: ap(e.mint),
                        relative: SOURCE,
                    },
                    ledger::ledger_lib::Child {
                        parent: ap(app),
                        relative: ap(relative),
                    },
                    1,
                )
                .unwrap();
            assert!(writable.contains(&ap(e.root_storage)));
            assert!(!writable.contains(&ap(e.mint)));
        }

        // A physically authentic record cannot substitute its PDA as the
        // logical root, nor can copying the record onto the mint authenticate it.
        let mut stored = h.svm.get_account(&e.root_storage).unwrap();
        assert!(decode_data(&ap(e.mint), &ledger::ID, &stored.data).is_err());
        let mut forged = h.record(e.root);
        forged.root = ap(e.root_storage);
        anchor_lang::AnchorSerialize::serialize(&forged, &mut &mut stored.data[8..]).unwrap();
        h.svm.set_account(e.root_storage, stored).unwrap();
        let withdrawal = e.movement(&h, (h.key(0), 1), (app, relative), 1, false, &[]);
        rejects(
            &mut h,
            &[0, 1],
            withdrawal,
            LedgerError::InvalidAccount.into(),
        );
    }
}
