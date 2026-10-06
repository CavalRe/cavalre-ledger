//! Ledger identity is its PDA storage address, throughout the tree and views.
use super::*;
use ledger::ledger_lib::{decode_data, global_root_address};
use ledger::ledger_view::Reader;
#[test]
fn ledger_pdas_are_used_in_storage_discovery_parent_links_and_posting() {
    for program in [TOKEN, TOKEN_2022] {
        let mut h = Harness::new();
        let e = External::setup(&mut h, 170, 1, program);
        let mint_before = h.svm.get_account(&e.mint).unwrap();
        assert_eq!(e.root, e.root_storage);
        assert_ne!(e.root, e.mint);
        let registration = e.registration(&h);
        succeeds(&mut h, &[0], registration.clone());
        let repeated = run(&mut h, &[0], registration).unwrap();
        assert!(events::event_bytes(&repeated.logs).is_empty());
        assert_eq!(
            h.record(e.root).ledger.as_ref().unwrap().identifier,
            ap(e.mint)
        );
        let global = sa(global_root_address().0);
        assert_eq!(h.record(global).children, vec![ap(e.mint)]);
        let app = branch(&mut h, e.root, 0, true);
        assert_eq!(h.record(app).parent, ap(e.root));
        assert_eq!(h.record(e.source).parent, ap(e.root));
        let relative = sa(ledger::ledger_lib::name_to_address("Arbitrary identity").unwrap());
        let create = leaf(&h, e.root, h.key(0), app, relative, "", false);
        succeeds(&mut h, &[0], create);
        let deposit = e.movement(&h, (h.key(0), 1), (app, relative), 10, true, &[]);
        succeeds(&mut h, &[0, 1], deposit);
        let position = child(app, relative);
        assert_eq!(h.record(position).custodian, ap(app));
        assert_eq!(h.svm.get_account(&e.mint).unwrap(), mint_before);
        assert!(h.svm.get_account(&relative).is_none());
        for reverse in [false, true] {
            let mut keys = vec![e.root, e.mint, app, position, e.source];
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
            assert_eq!(reader.total_supply(&ap(e.root)), Ok(10));
            assert_eq!(reader.ledger(&ap(position)), Ok(Some(ap(e.root))));
            assert_eq!(reader.known_ledgers(), Ok(vec![ap(e.root)]));
            assert_eq!(
                reader.total_supply(&ap(e.mint)),
                Err(ledger::ledger_lib::Error::MissingAccount)
            );
        }
        let saved = h.svm.get_account(&e.root).unwrap();
        assert!(decode_data(&ap(e.mint), &ledger::ID, &saved.data).is_err());
        let mut forged = saved.clone();
        forged.data[ledger::ledger_storage::IDENTIFIER] ^= 1;
        h.svm.set_account(e.root, forged).unwrap();
        let withdrawal = e.movement(&h, (h.key(0), 1), (app, relative), 1, false, &[]);
        rejects(
            &mut h,
            &[0, 1],
            withdrawal,
            LedgerError::InvalidAccount.into(),
        );
    }
}
