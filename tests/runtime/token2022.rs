//! Token-2022 admission and custody against LiteSVM's actual token-program sBPF.
use super::*;
use spl_token_2022_interface::{
    extension::{
        confidential_transfer::ConfidentialTransferMint, immutable_owner::ImmutableOwner,
        interest_bearing_mint::InterestBearingConfig, memo_transfer::MemoTransfer,
        permanent_delegate::PermanentDelegate, scaled_ui_amount::ScaledUiAmountConfig,
        transfer_fee::TransferFeeConfig, transfer_hook::TransferHook, BaseStateWithExtensions,
        BaseStateWithExtensionsMut, ExtensionType, StateWithExtensions, StateWithExtensionsMut,
    },
    instruction as token_ix,
    state::{Account as Account2022, Mint as Mint2022},
};

fn data(h: &mut Harness, address: Address, bytes: Vec<u8>) {
    let mut account = h.svm.get_account(&address).unwrap();
    account.lamports = h.svm.minimum_balance_for_rent_exemption(bytes.len()) + 10_000_000;
    account.data = bytes;
    h.svm.set_account(address, account).unwrap();
}

// Initialize mint, optional metadata, immutable wallet and supply through Token-2022.
pub(super) fn initialized(h: &mut Harness, metadata: bool) -> External {
    let e = External::setup(h, 60, 1, TOKEN_2022);
    let program = ap(TOKEN_2022);
    let mint = ap(e.mint);
    let wallet = ap(e.wallet);
    let authority = ap(h.key(0));
    let extensions = if metadata {
        vec![ExtensionType::MetadataPointer]
    } else {
        vec![]
    };
    let size = ExtensionType::try_calculate_account_len::<Mint2022>(&extensions).unwrap();
    data(h, e.mint, vec![0; size]);
    if metadata {
        let i = spl_token_2022_interface::extension::metadata_pointer::instruction::initialize(
            &program,
            &mint,
            Some(authority),
            Some(mint),
        )
        .unwrap();
        succeeds(h, &[0], i);
    }
    succeeds(
        h,
        &[0],
        token_ix::initialize_mint2(&program, &mint, &authority, Some(&authority), 6).unwrap(),
    );
    if metadata {
        succeeds(
            h,
            &[0],
            spl_token_metadata_interface::instruction::initialize(
                &program,
                &mint,
                &authority,
                &mint,
                &authority,
                "Metadata token".into(),
                "META".into(),
                "https://example.invalid/token".into(),
            ),
        );
        let account = h.svm.get_account(&e.mint).unwrap();
        let state = StateWithExtensions::<Mint2022>::unpack(&account.data).unwrap();
        assert!(state
            .get_extension_types()
            .unwrap()
            .contains(&ExtensionType::TokenMetadata));
    }
    let size =
        ExtensionType::try_calculate_account_len::<Account2022>(&[ExtensionType::ImmutableOwner])
            .unwrap();
    data(h, e.wallet, vec![0; size]);
    succeeds(
        h,
        &[0],
        token_ix::initialize_immutable_owner(&program, &wallet).unwrap(),
    );
    succeeds(
        h,
        &[0],
        token_ix::initialize_account3(&program, &wallet, &mint, &ap(h.key(1))).unwrap(),
    );
    succeeds(
        h,
        &[0],
        token_ix::mint_to_checked(&program, &mint, &wallet, &authority, &[], 1000, 6).unwrap(),
    );
    e
}

#[test]
fn plain_and_metadata_tokens_settle_directly_and_through_cpi() {
    for metadata in [false, true] {
        for cpi in [false, true] {
            let mut h = Harness::new();
            let e = initialized(&mut h, metadata);
            let i = e.registration(&h);
            succeeds(&mut h, &[0], i);
            assert_eq!(h.svm.get_account(&e.vault).unwrap().owner, TOKEN_2022);
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
            let a = h.key(2);
            let b = Address::new_from_array([63; 32]);
            let i = call(
                &h,
                group(&h, e.root, authority, e.root, authority, true, &[]),
            );
            succeeds(&mut h, &[0], i);
            let i = call(
                &h,
                e.movement(&h, (authority, 0), (parent, a), 100, true, &[]),
            );
            rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
            let i = call(
                &h,
                e.movement(&h, (authority, 1), (parent, a), 100, true, &[]),
            );
            let result = run(&mut h, &[0, 1], i).unwrap();
            assert!(result
                .logs
                .iter()
                .any(|line| line.contains(&format!("Program {TOKEN_2022} invoke"))));
            assert_eq!((h.token(e.wallet), h.token(e.vault)), (900, 100));
            assert!(h.record(child(parent, a)).child_index > 0);
            let i = call(
                &h,
                transfer(&h, e.root, authority, (parent, a), (parent, b), 40, &[]),
            );
            succeeds(&mut h, &[0], i);
            let i = call(
                &h,
                e.movement(&h, (authority, 0), (parent, b), 40, false, &[]),
            );
            succeeds(&mut h, &[0], i); // Neither token recipient nor leaf owner signs.
            assert_eq!((h.token(e.wallet), h.token(e.vault)), (940, 60));
            assert_eq!(
                (
                    h.record(child(parent, a)).debit,
                    h.record(child(parent, b)).debit
                ),
                (60, 0)
            );
            assert_eq!(
                (
                    h.record(e.root).debit,
                    h.record(e.root).credit,
                    h.record(e.source).credit
                ),
                (60, 60, 60)
            );
        }
    }
}

#[test]
fn metadata_views_execute_in_a_consumer_without_calling_ledger() {
    use anchor_lang::AnchorDeserialize;
    let mut h = Harness::new();
    let e = initialized(&mut h, true);
    let i = e.registration(&h);
    succeeds(&mut h, &[0], i);
    let consumer = Address::new_from_array([62; 32]);
    h.svm
        .add_program(
            consumer,
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
            )
            .unwrap(),
        )
        .unwrap();
    let read = Instruction {
        program_id: consumer,
        accounts: vec![
            AccountMeta::new_readonly(e.root_storage, false),
            AccountMeta::new_readonly(h.metadata_key(e.root), false),
        ],
        data: [b"metadata".as_slice(), e.root.as_ref()].concat(),
    };
    for symbol in ["META", "UPDATED"] {
        if symbol == "UPDATED" {
            let ix = spl_token_metadata_interface::instruction::update_field(
                &ap(TOKEN_2022),
                &ap(e.mint),
                &ap(h.key(0)),
                spl_token_metadata_interface::state::Field::Symbol,
                symbol.into(),
            );
            succeeds(&mut h, &[0], ix);
        }
        let before = (
            h.svm.get_account(&e.root_storage).unwrap(),
            h.svm.get_account(&e.mint).unwrap(),
        );
        let result = run(&mut h, &[0], read.clone()).unwrap();
        assert_eq!(result.return_data.program_id, consumer);
        let fields =
            <(Option<String>, Option<u8>)>::deserialize(&mut &result.return_data.data[..]).unwrap();
        assert_eq!(fields, (Some("META".into()), Some(6)));
        assert_eq!(h.labels(e.root).name, "Metadata token");
        assert!(!result
            .logs
            .iter()
            .any(|line| line.contains(&format!("Program {} invoke", cavalre_ledger_solana::ID))));
        assert_eq!(
            (
                h.svm.get_account(&e.root_storage).unwrap(),
                h.svm.get_account(&e.mint).unwrap()
            ),
            before
        );
    }
    // Stored metadata is readable at a non-root account and defaults at a
    // confirmed absent account, without invoking Ledger or allocating a record.
    let absent = child(e.root, h.key(2));
    assert!(h.svm.get_account(&absent).is_none());
    let ledger_metadata = h.metadata_key(e.root);
    for target in [e.source, absent] {
        let before = [target, e.root_storage].map(|key| h.svm.get_account(&key));
        let result = run(
            &mut h,
            &[0],
            Instruction {
                program_id: consumer,
                accounts: vec![
                    AccountMeta::new_readonly(target, false),
                    AccountMeta::new_readonly(e.root_storage, false),
                    AccountMeta::new_readonly(ledger_metadata, false),
                ],
                data: [b"metadata".as_slice(), target.as_ref()].concat(),
            },
        )
        .unwrap();
        let fields =
            <(Option<String>, Option<u8>)>::deserialize(&mut &result.return_data.data[..]).unwrap();
        assert_eq!(
            fields,
            if target == absent {
                (Some(String::new()), Some(0))
            } else {
                (Some("META".into()), Some(6))
            }
        );
        assert_eq!(result.return_data.program_id, consumer);
        assert!(!result
            .logs
            .iter()
            .any(|line| line.contains(&format!("Program {} invoke", cavalre_ledger_solana::ID))));
        assert_eq!(
            [target, e.root_storage].map(|key| h.svm.get_account(&key)),
            before
        );
    }
}

// Deliberately constructed extension states exercise admission, not mint permissions.
fn mint_extension(h: &mut Harness, e: &External, extension: ExtensionType) {
    let base =
        Mint2022::unpack(&h.svm.get_account(&e.mint).unwrap().data[..Mint2022::LEN]).unwrap();
    let mut bytes =
        vec![0; ExtensionType::try_calculate_account_len::<Mint2022>(&[extension]).unwrap()];
    let mut state = StateWithExtensionsMut::<Mint2022>::unpack_uninitialized(&mut bytes).unwrap();
    match extension {
        ExtensionType::TransferFeeConfig => {
            state.init_extension::<TransferFeeConfig>(false).unwrap();
        }
        ExtensionType::TransferHook => {
            state.init_extension::<TransferHook>(false).unwrap();
        }
        ExtensionType::PermanentDelegate => {
            state.init_extension::<PermanentDelegate>(false).unwrap();
        }
        ExtensionType::ConfidentialTransferMint => {
            state
                .init_extension::<ConfidentialTransferMint>(false)
                .unwrap();
        }
        ExtensionType::InterestBearingConfig => {
            state
                .init_extension::<InterestBearingConfig>(false)
                .unwrap();
        }
        ExtensionType::ScaledUiAmount => {
            let config = state.init_extension::<ScaledUiAmountConfig>(false).unwrap();
            config.multiplier = 2.0.into();
            config.new_multiplier = 2.0.into();
        }
        _ => panic!("missing fixture for {extension:?}"),
    }
    state.base = base;
    state.pack_base();
    state.init_account_type().unwrap();
    data(h, e.mint, bytes);
}

#[test]
fn incompatible_mints_reject_registration_without_allocating_ledger_state() {
    for extension in [
        ExtensionType::TransferFeeConfig,
        ExtensionType::PermanentDelegate,
        ExtensionType::ConfidentialTransferMint,
    ] {
        let mut h = Harness::new();
        let e = External::setup(&mut h, 60, 1, TOKEN_2022);
        mint_extension(&mut h, &e, extension);
        let i = e.registration(&h);
        rejects(&mut h, &[0], i, LedgerError::UnsupportedToken.into());
        for address in [e.root_storage, e.source, e.vault] {
            assert!(h.svm.get_account(&address).is_none());
        }
    }
}

#[test]
fn ui_amount_extensions_preserve_raw_unit_settlement() {
    for extension in [
        ExtensionType::InterestBearingConfig,
        ExtensionType::ScaledUiAmount,
    ] {
        let mut h = Harness::new();
        let e = External::setup(&mut h, 60, 1, TOKEN_2022);
        mint_extension(&mut h, &e, extension);
        let i = e.registration(&h);
        succeeds(&mut h, &[0], i);
        let parent = branch(&mut h, e.root, 0, true);
        let user = h.key(2);
        let i = e.movement(&h, (h.key(0), 1), (parent, user), 100, true, &[]);
        succeeds(&mut h, &[0, 1], i);
        assert_eq!(
            (h.token(e.vault), h.record(child(parent, user)).debit),
            (100, 100)
        );
        let i = e.movement(&h, (h.key(0), 0), (parent, user), 100, false, &[]);
        succeeds(&mut h, &[0], i);
        assert_eq!(
            (h.token(e.wallet), h.token(e.vault), h.record(e.root).credit),
            (1000, 0, 0)
        );
    }
}

#[test]
fn mint_and_account_extensions_are_rechecked_on_each_settlement() {
    let mut h = Harness::new();
    let e = initialized(&mut h, false);
    let i = e.registration(&h);
    succeeds(&mut h, &[0], i);
    let parent = branch(&mut h, e.root, 0, true);
    let user = h.key(2);
    let i = e.movement(&h, (h.key(0), 1), (parent, user), 100, true, &[]);
    succeeds(&mut h, &[0, 1], i);
    let original = h.svm.get_account(&e.mint).unwrap();
    mint_extension(&mut h, &e, ExtensionType::PermanentDelegate);
    for deposit in [false, true] {
        let i = e.movement(&h, (h.key(0), 1), (parent, user), 1, deposit, &[]);
        rejects(&mut h, &[0, 1], i, LedgerError::UnsupportedToken.into());
    }
    h.svm.set_account(e.mint, original).unwrap();
    for address in [e.wallet, e.vault] {
        let original = h.svm.get_account(&address).unwrap();
        let base = Account2022::unpack(&original.data[..Account2022::LEN]).unwrap();
        let mut bytes = vec![
            0;
            ExtensionType::try_calculate_account_len::<Account2022>(&[
                ExtensionType::ImmutableOwner,
                ExtensionType::MemoTransfer
            ])
            .unwrap()
        ];
        let mut state =
            StateWithExtensionsMut::<Account2022>::unpack_uninitialized(&mut bytes).unwrap();
        state.init_extension::<ImmutableOwner>(false).unwrap();
        state
            .init_extension::<MemoTransfer>(false)
            .unwrap()
            .require_incoming_transfer_memos = true.into();
        state.base = base;
        state.pack_base();
        state.init_account_type().unwrap();
        data(&mut h, address, bytes);
        for deposit in [false, true] {
            let i = e.movement(&h, (h.key(0), 1), (parent, user), 1, deposit, &[]);
            rejects(&mut h, &[0, 1], i, LedgerError::UnsupportedToken.into());
        }
        h.svm.set_account(address, original).unwrap();
    }
}

#[test]
fn token_program_must_match_mint_vault_and_wallet() {
    let mut h = Harness::new();
    let e = initialized(&mut h, false);
    let mut i = e.registration(&h);
    for account in &mut i.accounts {
        if account.pubkey == TOKEN_2022 {
            account.pubkey = TOKEN;
        }
    }
    // Anchor initializes the vault before running non-init constraints. The
    // selected token program rejects the foreign mint; all allocation rolls back.
    rejects_with_error(&mut h, &[0], i, InstructionError::IncorrectProgramId);
    let i = e.registration(&h);
    succeeds(&mut h, &[0], i);
    let parent = branch(&mut h, e.root, 0, true);
    let user = h.key(2);
    let valid = e.movement(&h, (h.key(0), 1), (parent, user), 100, true, &[]);
    let mut i = valid.clone();
    for account in &mut i.accounts {
        if account.pubkey == TOKEN_2022 {
            account.pubkey = TOKEN;
        }
    }
    rejects(
        &mut h,
        &[0, 1],
        i,
        ErrorCode::ConstraintMintTokenProgram.into(),
    );
    for address in [e.wallet, e.vault] {
        let original = h.svm.get_account(&address).unwrap();
        let mut substituted = original.clone();
        substituted.owner = TOKEN;
        h.svm.set_account(address, substituted).unwrap();
        rejects(
            &mut h,
            &[0, 1],
            valid.clone(),
            ErrorCode::ConstraintTokenTokenProgram.into(),
        );
        h.svm.set_account(address, original).unwrap();
    }
}

#[test]
fn failed_token2022_settlement_and_late_commit_roll_back() {
    let mut h = Harness::new();
    let e = initialized(&mut h, true);
    let i = e.registration(&h);
    succeeds(&mut h, &[0], i);
    let parent = branch(&mut h, e.root, 0, true);
    let user = h.key(2);
    let valid = e.movement(&h, (h.key(0), 1), (parent, user), 100, true, &[]);
    let freeze = token_ix::freeze_account(
        &ap(TOKEN_2022),
        &ap(e.wallet),
        &ap(e.mint),
        &ap(h.key(0)),
        &[],
    )
    .unwrap();
    succeeds(&mut h, &[0], freeze);
    rejects(
        &mut h,
        &[0, 1],
        valid.clone(),
        spl_token_2022_interface::error::TokenError::AccountFrozen as u32,
    );
    let thaw = token_ix::thaw_account(
        &ap(TOKEN_2022),
        &ap(e.wallet),
        &ap(e.mint),
        &ap(h.key(0)),
        &[],
    )
    .unwrap();
    succeeds(&mut h, &[0], thaw);
    let mut readonly = valid.clone();
    for account in &mut readonly.accounts {
        if account.pubkey == parent {
            account.is_writable = false;
        }
    }
    rejects(
        &mut h,
        &[0, 1],
        readonly,
        LedgerError::InvalidAccount.into(),
    );
    assert!(h.svm.get_account(&child(parent, user)).is_none());
    assert_eq!((h.token(e.wallet), h.token(e.vault)), (1000, 0));
    succeeds(&mut h, &[0, 1], valid);
}
