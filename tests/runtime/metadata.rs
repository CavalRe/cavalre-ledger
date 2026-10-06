//! Issuer metadata snapshots and idempotent registration through actual sBPF.
use super::*;
use ledger::ledger_view::{metadata_address, Reader};

fn assert_noop(h: &mut Harness, instruction: Instruction) {
    let instruction = h.indexed(instruction);
    let before: Vec<_> = instruction
        .accounts
        .iter()
        .map(|m| (m.pubkey, h.account(&m.pubkey)))
        .collect();
    let result = run_raw(h, &[0], instruction).unwrap();
    assert!(events::event_bytes(&result.logs).is_empty());
    for (key, old) in before {
        let mut current = h.account(&key);
        if key == h.key(0) {
            current.as_mut().unwrap().lamports += result.fee;
        }
        assert_eq!(current, old, "repeat registration wrote or allocated {key}");
    }
}

#[test]
fn external_registration_snapshots_issuer_metadata_and_repeats_are_noops() {
    for program in [TOKEN, TOKEN_2022] {
        let mut h = Harness::new();
        let e = External::setup(&mut h, 60, 0, program);
        h.metadata(e.mint, "USD Coin\0\0", "USDC\0\0");
        let registration = e.registration(&h);
        assert_eq!(
            registration.data.len(),
            8,
            "no caller-supplied metadata arguments"
        );
        succeeds(&mut h, &[0], registration.clone());
        let record = h.record(e.root);
        assert_eq!(
            (
                record.name.as_str(),
                record.symbol.as_str(),
                record.decimals
            ),
            ("USD Coin", "USDC", 6)
        );
        assert_noop(&mut h, registration.clone());
        let authority = h.key(0);
        let deposit = e.movement(&h, (authority, 0), (e.root, authority), 10, true, &[]);
        succeeds(&mut h, &[0], deposit);
        assert_noop(&mut h, registration.clone());
        assert_eq!(
            (
                h.record(e.root).debit,
                h.record(e.source).credit,
                h.token(e.vault)
            ),
            (10, 10, 10)
        );
        h.metadata(e.mint, "New issuer name", "NEW");
        let stored = h.account(&e.root_storage).unwrap();
        let mut reader = Reader::new();
        reader
            .insert(ap(e.root_storage), &ap(stored.owner), &stored.data)
            .unwrap();
        assert_eq!(reader.name(&ap(e.root)).unwrap(), "USD Coin");
        assert_eq!(reader.symbol(&ap(e.root)).unwrap(), Some("USDC".into()));
        assert_eq!(reader.decimals(&ap(e.root)).unwrap(), Some(6));
        rejects(
            &mut h,
            &[0],
            registration,
            LedgerError::MetadataConflict.into(),
        );
        assert_eq!(h.account(&e.root_storage).unwrap(), stored);
    }
}

#[test]
fn registration_rejects_missing_empty_forged_or_malformed_issuer_metadata_atomically() {
    for program in [TOKEN, TOKEN_2022] {
        for case in [
            "missing",
            "owner",
            "mint",
            "header",
            "truncated",
            "empty-name",
            "empty-symbol",
            "long-name",
        ] {
            let mut h = Harness::new();
            let e = External::setup(&mut h, 60, 0, program);
            let key = sa(metadata_address(&ap(e.mint)));
            let mut registration = e.registration(&h);
            let expected = match case {
                "missing" => {
                    registration.accounts.retain(|m| m.pubkey != key);
                    LedgerError::MissingAccount
                }
                "empty-name" => {
                    h.metadata(e.mint, "", "TOK");
                    LedgerError::InvalidName
                }
                "empty-symbol" => {
                    h.metadata(e.mint, "Token", "");
                    LedgerError::InvalidName
                }
                "long-name" => {
                    h.metadata(e.mint, &"N".repeat(33), "TOK");
                    LedgerError::InvalidAccount
                }
                _ => {
                    let mut record = h.account(&key).unwrap();
                    match case {
                        "owner" => record.owner = SYSTEM,
                        "mint" => record.data[33..65].copy_from_slice(h.key(2).as_ref()),
                        "header" => record.data[0] = 1,
                        "truncated" => record.data.truncate(68),
                        _ => unreachable!(),
                    }
                    h.svm.set_account(key, record).unwrap();
                    LedgerError::InvalidAccount
                }
            };
            rejects(&mut h, &[0], registration, expected.into());
            for key in [
                e.root_storage,
                e.source,
                e.vault,
                sa(ledger::ledger_lib::global_root_address().0),
            ] {
                assert!(
                    h.account(&key).is_none(),
                    "failed {case} left allocated state"
                );
            }
        }
    }
}

#[test]
fn token2022_inline_names_are_issuer_authenticated_and_obey_original_string_limits() {
    for (field, value, valid) in [
        (
            spl_token_metadata_interface::state::Field::Name,
            "N".repeat(64),
            true,
        ),
        (
            spl_token_metadata_interface::state::Field::Symbol,
            "S".repeat(64),
            true,
        ),
        (
            spl_token_metadata_interface::state::Field::Name,
            "N".repeat(65),
            false,
        ),
        (
            spl_token_metadata_interface::state::Field::Symbol,
            "S".repeat(65),
            false,
        ),
        (
            spl_token_metadata_interface::state::Field::Name,
            String::new(),
            false,
        ),
        (
            spl_token_metadata_interface::state::Field::Symbol,
            String::new(),
            false,
        ),
    ] {
        let mut h = Harness::new();
        let e = token2022::initialized(&mut h, true);
        let name_field = matches!(field, spl_token_metadata_interface::state::Field::Name);
        let update = spl_token_metadata_interface::instruction::update_field(
            &ap(TOKEN_2022),
            &ap(e.mint),
            &ap(h.key(0)),
            field,
            value.clone(),
        );
        succeeds(&mut h, &[0], update);
        let mut registration = e.registration(&h);
        registration
            .accounts
            .retain(|m| m.pubkey != sa(metadata_address(&ap(e.mint))));
        if valid {
            succeeds(&mut h, &[0], registration.clone());
            let record = h.record(e.root);
            assert_eq!(
                if name_field {
                    record.name
                } else {
                    record.symbol
                },
                value
            );
            assert_noop(&mut h, registration);
        } else {
            rejects(&mut h, &[0], registration, LedgerError::InvalidName.into());
            assert!(h.account(&e.root_storage).is_none());
            assert!(h.account(&e.vault).is_none());
        }
    }
}

#[test]
fn registration_obeys_metadata_pointer_instead_of_stale_inline_labels() {
    for selection in ["inline", "metaplex", "missing", "undefined", "unsupported"] {
        let mut h = Harness::new();
        let e = token2022::initialized(&mut h, true);
        h.metadata(e.mint, "Selected issuer name", "ISSUER");
        let canonical = metadata_address(&ap(e.mint));
        let target = match selection {
            "inline" => Some(ap(e.mint)),
            "metaplex" | "missing" => Some(canonical),
            "undefined" => None,
            "unsupported" => Some(ap(h.key(2))),
            _ => unreachable!(),
        };
        let update = spl_token_2022_interface::extension::metadata_pointer::instruction::update(
            &ap(TOKEN_2022),
            &ap(e.mint),
            &ap(h.key(0)),
            &[],
            target,
        )
        .unwrap();
        succeeds(&mut h, &[0], update);
        let mut registration = e.registration(&h);
        if selection == "missing" {
            registration.accounts.retain(|m| m.pubkey != sa(canonical));
        }
        if matches!(selection, "inline" | "metaplex") {
            succeeds(&mut h, &[0], registration.clone());
            let record = h.record(e.root);
            let expected = if selection == "inline" {
                ("Metadata token", "META")
            } else {
                ("Selected issuer name", "ISSUER")
            };
            assert_eq!((record.name.as_str(), record.symbol.as_str()), expected);
            assert_noop(&mut h, registration);
        } else {
            let error = if selection == "missing" {
                LedgerError::MissingAccount
            } else {
                LedgerError::InvalidAccount
            };
            rejects(&mut h, &[0], registration, error.into());
            assert!(h.account(&e.root_storage).is_none());
            assert!(h.account(&e.vault).is_none());
        }
    }
}

#[test]
fn accounting_ledger_metadata_is_explicit_and_matching_creation_is_idempotent() {
    let mut h = Harness::new();
    let root = sa(ledger::ledger_lib::root_storage_address(&ap(h.key(0)), &ap(h.key(2))).0);
    let build = |h: &Harness, name: &str, symbol: &str, decimals| {
        ix(
            h.registration(0, root),
            instruction::AddLedger {
                id: ap(h.key(2)),
                name: name.into(),
                symbol: symbol.into(),
                decimals,
            },
            &[child(root, sa(SOURCE))],
        )
    };
    let i = build(&h, "Epoch receipts", "EPOCH", 0);
    succeeds(&mut h, &[0], i.clone());
    assert_noop(&mut h, i);
    let record = h.record(root);
    assert_eq!(
        (
            record.name.as_str(),
            record.symbol.as_str(),
            record.decimals
        ),
        ("Epoch receipts", "EPOCH", 0)
    );
    for (name, symbol, decimals) in [
        ("Other", "EPOCH", 0),
        ("Epoch receipts", "OTHER", 0),
        ("Epoch receipts", "EPOCH", 1),
    ] {
        let i = build(&h, name, symbol, decimals);
        rejects(&mut h, &[0], i, LedgerError::MetadataConflict.into());
    }
}
