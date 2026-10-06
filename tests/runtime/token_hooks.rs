//! Real Token-2022 and hook sBPF; no native processor substitutes.
use super::*;
use spl_tlv_account_resolution::{account::ExtraAccountMeta, state::ExtraAccountMetaList};
use spl_transfer_hook_interface::instruction::ExecuteInstruction;
use token2022_current::{
    extension::{ExtensionType, StateWithExtensions},
    instruction as token_ix,
    state::{Account as TokenRecord, Mint as MintRecord},
};

const HOOK: Address = Address::new_from_array([79; 32]);
const APP: Address = Address::new_from_array([62; 32]);
fn artifact() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
    )
    .unwrap()
}
fn hook_state(mint: Address) -> Address {
    Address::find_program_address(&[b"hook-state", mint.as_ref()], &HOOK).0
}
fn hook_metadata(mint: Address) -> Address {
    spl_transfer_hook_interface::get_extra_account_metas_address(&mint, &HOOK)
}
fn app_authority(h: &Harness) -> Address {
    Address::find_program_address(&[b"app", h.key(0).as_ref()], &APP).0
}
fn replace_data(h: &mut Harness, key: Address, bytes: Vec<u8>, owner: Address) {
    h.svm
        .set_account(
            key,
            Account {
                lamports: h.svm.minimum_balance_for_rent_exemption(bytes.len()) + 10_000_000,
                data: bytes,
                owner,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
}
fn program_token(h: &Harness, e: &External, amount: u64, burn: bool) -> Instruction {
    let mut data = if burn {
        b"token-burn".to_vec()
    } else {
        b"token-mint".to_vec()
    };
    data.extend(amount.to_le_bytes());
    Instruction {
        program_id: APP,
        accounts: vec![
            AccountMeta::new(h.key(0), true),
            AccountMeta::new_readonly(TOKEN_2022, false),
            AccountMeta::new(e.mint, false),
            AccountMeta::new(e.wallet, false),
            AccountMeta::new_readonly(app_authority(h), false),
        ],
        data,
    }
}
// hook: None means no extension; Some(false) is disabled; Some(true) is active.
fn initialized(
    h: &mut Harness,
    hook: Option<bool>,
    burn: bool,
    program_controlled: bool,
) -> External {
    let e = External::setup(h, 60, 1, TOKEN_2022);
    h.svm.add_program(HOOK, &artifact()).unwrap();
    h.svm.add_program(APP, &artifact()).unwrap();
    let mint_authority = if program_controlled {
        app_authority(h)
    } else {
        h.key(0)
    };
    let owner = if program_controlled {
        app_authority(h)
    } else {
        h.key(1)
    };
    let burn_authority = if program_controlled {
        app_authority(h)
    } else {
        h.key(2)
    };
    let mut extensions = vec![];
    if hook.is_some() {
        extensions.push(ExtensionType::TransferHook);
    }
    if burn {
        extensions.push(ExtensionType::PermissionedBurn);
    }
    let inline_metadata = hook == Some(true) && burn;
    if inline_metadata {
        extensions.push(ExtensionType::MetadataPointer);
    }
    let size = ExtensionType::try_calculate_account_len::<MintRecord>(&extensions).unwrap();
    replace_data(h, e.mint, vec![0; size], TOKEN_2022);
    if inline_metadata {
        let ix = token2022_current::extension::metadata_pointer::instruction::initialize(
            &TOKEN_2022,
            &e.mint,
            Some(mint_authority),
            Some(e.mint),
        )
        .unwrap();
        succeeds(h, &[0], ix);
    }
    if let Some(enabled) = hook {
        let ix = token2022_current::extension::transfer_hook::instruction::initialize(
            &TOKEN_2022,
            &e.mint,
            Some(h.key(0)),
            enabled.then_some(HOOK),
        )
        .unwrap();
        succeeds(h, &[0], ix);
    }
    if burn {
        let ix = token2022_current::extension::permissioned_burn::instruction::initialize(
            &TOKEN_2022,
            &e.mint,
            &burn_authority,
        )
        .unwrap();
        succeeds(h, &[0], ix);
    }
    succeeds(
        h,
        &[0],
        token_ix::initialize_mint2(&TOKEN_2022, &e.mint, &mint_authority, None, 6).unwrap(),
    );
    if inline_metadata {
        succeeds(
            h,
            &[0],
            spl_token_metadata_interface::instruction::initialize(
                &TOKEN_2022,
                &e.mint,
                &mint_authority,
                &e.mint,
                &mint_authority,
                "Hook token".into(),
                "HOOK".into(),
                "https://example.invalid/hook".into(),
            ),
        );
    }
    let mut account_extensions = vec![ExtensionType::ImmutableOwner];
    if hook.is_some() {
        account_extensions.push(ExtensionType::TransferHookAccount);
    }
    let size =
        ExtensionType::try_calculate_account_len::<TokenRecord>(&account_extensions).unwrap();
    replace_data(h, e.wallet, vec![0; size], TOKEN_2022);
    succeeds(
        h,
        &[0],
        token_ix::initialize_immutable_owner(&TOKEN_2022, &e.wallet).unwrap(),
    );
    succeeds(
        h,
        &[0],
        token_ix::initialize_account3(&TOKEN_2022, &e.wallet, &e.mint, &owner).unwrap(),
    );
    if program_controlled {
        let ix = program_token(h, &e, 1000, false);
        succeeds(h, &[0], ix);
    } else {
        succeeds(
            h,
            &[0],
            token_ix::mint_to_checked(
                &TOKEN_2022,
                &e.mint,
                &e.wallet,
                &mint_authority,
                &[],
                1000,
                6,
            )
            .unwrap(),
        );
    }
    let extras = vec![
        ExtraAccountMeta::new_with_pubkey(&hook_state(e.mint), false, true).unwrap(),
        ExtraAccountMeta::new_with_pubkey(&ledger::ID, false, false).unwrap(),
    ];
    let mut bytes = vec![0; ExtraAccountMetaList::size_of(extras.len()).unwrap()];
    ExtraAccountMetaList::init::<ExecuteInstruction>(&mut bytes, &extras).unwrap();
    replace_data(h, hook_metadata(e.mint), bytes, HOOK);
    replace_data(h, hook_state(e.mint), vec![0; 17], HOOK);
    e
}
fn extras(e: &External, mut ix: Instruction) -> Instruction {
    for meta in [
        AccountMeta::new_readonly(hook_metadata(e.mint), false),
        AccountMeta::new(hook_state(e.mint), false),
        AccountMeta::new_readonly(HOOK, false),
        AccountMeta::new_readonly(ledger::ID, false),
    ] {
        if !ix.accounts.iter().any(|m| m.pubkey == meta.pubkey) {
            ix.accounts.push(meta);
        }
    }
    ix
}
fn count(h: &Harness, e: &External) -> u64 {
    u64::from_le_bytes(
        h.account(&hook_state(e.mint)).unwrap().data[1..9]
            .try_into()
            .unwrap(),
    )
}
fn mode(h: &mut Harness, e: &External, mode: u8) {
    let mut state = h.account(&hook_state(e.mint)).unwrap();
    state.data[0] = mode;
    h.svm.set_account(hook_state(e.mint), state).unwrap();
}
fn reject_unchanged(
    h: &mut Harness,
    signers: &[usize],
    ix: Instruction,
) -> FailedTransactionMetadata {
    reject_many_unchanged(h, signers, vec![h.indexed(ix)])
}
fn run_many(
    h: &mut Harness,
    signers: &[usize],
    instructions: &[Instruction],
) -> Result<TransactionMetadata, Box<FailedTransactionMetadata>> {
    h.svm.expire_blockhash();
    let keys: Vec<_> = signers.iter().map(|i| &h.keys[*i]).collect();
    let tx = Transaction::new_signed_with_payer(
        instructions,
        Some(&h.key(0)),
        &keys,
        h.svm.latest_blockhash(),
    );
    h.svm.send_transaction(tx).map_err(Box::new)
}
fn reject_many_unchanged(
    h: &mut Harness,
    signers: &[usize],
    instructions: Vec<Instruction>,
) -> FailedTransactionMetadata {
    let before: Vec<_> = instructions
        .iter()
        .flat_map(|ix| ix.accounts.iter())
        .map(|m| (m.pubkey, h.account(&m.pubkey)))
        .collect();
    let failure = *run_many(h, signers, &instructions).expect_err("operation must reject");
    for (key, old) in before {
        let mut now = h.account(&key);
        if key == h.key(0) {
            now.as_mut().unwrap().lamports += failure.meta.fee;
        }
        assert_eq!(now, old, "rollback failed for {key}");
    }
    failure
}

#[test]
fn hooks_and_permissioned_burn_settle_directly_and_through_application_cpi() {
    let mut rows = vec![];
    for (hook, burn) in [
        (None, false),
        (None, true),
        (Some(false), true),
        (Some(true), false),
        (Some(true), true),
    ] {
        for cpi in [false, true] {
            let mut h = Harness::new();
            let e = initialized(&mut h, hook, burn, false);
            let init = e.registration(&h);
            let result = run(&mut h, &[0], init.clone()).unwrap();
            rows.push(serde_json::json!({"hook":hook,"burn":burn,"cpi":cpi,"operation":"register","compute_units":result.compute_units_consumed,"custody_bytes":h.account(&e.vault).unwrap().data.len()}));
            let authority = if cpi { app_authority(&h) } else { h.key(0) };
            let parent = child(e.root, authority);
            let holder = h.key(2);
            let call = |h: &Harness, ix| {
                if cpi {
                    proxy(h, APP, authority, ix)
                } else {
                    ix
                }
            };
            let ix = call(
                &h,
                group(&h, e.root, authority, e.root, authority, true, &[]),
            );
            succeeds(&mut h, &[0], ix);
            for (deposit, amount) in [(true, 100), (true, 0), (false, 30), (false, 0)] {
                let ix = e.movement(&h, (authority, 1), (parent, holder), amount, deposit, &[]);
                let ix = if hook == Some(true) {
                    extras(&e, ix)
                } else {
                    ix
                };
                let ix = call(&h, ix);
                let bytes = wincode::serialize(&Transaction::new_signed_with_payer(
                    &[h.indexed(ix.clone())],
                    Some(&h.key(0)),
                    &[&h.keys[0], &h.keys[1]],
                    h.svm.latest_blockhash(),
                ))
                .unwrap()
                .len();
                let result = run(&mut h, &[0, 1], ix).unwrap();
                rows.push(serde_json::json!({"hook":hook,"burn":burn,"cpi":cpi,"operation":if deposit{"wrap"}else{"unwrap"},"amount":amount,"compute_units":result.compute_units_consumed,"transaction_bytes":bytes}));
            }
            assert_eq!((h.token(e.wallet), h.token(e.vault)), (930, 70));
            assert_eq!(
                (
                    h.record(e.root).debit,
                    h.record(e.root).credit,
                    h.record(e.source).credit
                ),
                (70, 70, 70)
            );
            assert_eq!(h.record(child(parent, holder)).debit, 70);
            assert_eq!(count(&h, &e), if hook == Some(true) { 4 } else { 0 });
            let before = h.account(&e.root_storage).unwrap();
            succeeds(&mut h, &[0], init);
            assert_eq!(h.account(&e.root_storage).unwrap(), before);
            // Internal postings observe backing and do not call a token hook.
            let ix = call(
                &h,
                transfer(
                    &h,
                    e.root,
                    authority,
                    (parent, holder),
                    (parent, h.key(1)),
                    10,
                    &[],
                ),
            );
            succeeds(&mut h, &[0], ix);
            assert_eq!(count(&h, &e), if hook == Some(true) { 4 } else { 0 });
        }
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    std::fs::write(
        dir.join("token-hook-costs.json"),
        serde_json::to_string_pretty(&rows).unwrap(),
    )
    .unwrap();
}

#[test]
fn hook_failures_missing_records_and_reentrancy_roll_back_token_and_ledger_changes() {
    let mut h = Harness::new();
    let e = initialized(&mut h, Some(true), true, false);
    let init = e.registration(&h);
    succeeds(&mut h, &[0], init);
    let parent = branch(&mut h, e.root, 0, true);
    let holder = h.key(2);
    let call = |h: &Harness, deposit, amount| {
        extras(
            &e,
            e.movement(h, (h.key(0), 1), (parent, holder), amount, deposit, &[]),
        )
    };
    let missing_extra = InstructionError::Custom(
        spl_tlv_account_resolution::error::AccountResolutionError::IncorrectAccount as u32,
    );
    // The pinned runtime maps ProgramError::NotEnoughAccountKeys to this legacy variant.
    #[allow(deprecated)]
    let missing_keys = InstructionError::NotEnoughAccountKeys;
    for (missing, expected) in [
        (
            HOOK,
            InstructionError::Custom(
                spl_transfer_hook_interface::error::TransferHookError::IncorrectAccount as u32,
            ),
        ),
        (hook_state(e.mint), missing_extra.clone()),
        (hook_metadata(e.mint), missing_keys),
    ] {
        let mut ix = call(&h, true, 100);
        ix.accounts.retain(|m| m.pubkey != missing);
        let failed = reject_unchanged(&mut h, &[0, 1], ix);
        assert_eq!(failed.err, TransactionError::InstructionError(0, expected));
    }
    let fake = Address::new_from_array([81; 32]);
    h.svm
        .set_account(fake, h.account(&hook_state(e.mint)).unwrap())
        .unwrap();
    let mut ix = call(&h, true, 100);
    for m in &mut ix.accounts {
        if m.pubkey == hook_state(e.mint) {
            m.pubkey = fake;
        }
    }
    let failed = reject_unchanged(&mut h, &[0, 1], ix);
    assert_eq!(
        failed.err,
        TransactionError::InstructionError(0, missing_extra)
    );
    mode(&mut h, &e, 1);
    let ix = call(&h, true, 100);
    rejects(&mut h, &[0, 1], ix, 7101);
    assert!(h.maybe_record(child(parent, holder)).is_none());
    mode(&mut h, &e, 0);
    let ix = call(&h, true, 100);
    succeeds(&mut h, &[0, 1], ix);
    mode(&mut h, &e, 1);
    let ix = call(&h, false, 30);
    rejects(&mut h, &[0, 1], ix, 7101);
    mode(&mut h, &e, 2);
    let ix = call(&h, false, 30);
    let failed = reject_unchanged(&mut h, &[0, 1], ix);
    assert_eq!(
        failed.err,
        TransactionError::InstructionError(0, InstructionError::ReentrancyNotAllowed)
    );
    mode(&mut h, &e, 0);
    let mut ix = call(&h, true, 5);
    ix.accounts[3].is_writable = false;
    let failed = reject_unchanged(&mut h, &[0, 1], ix);
    assert_eq!(
        failed.err,
        TransactionError::InstructionError(
            0,
            InstructionError::Custom(LedgerError::InvalidAccount.into())
        )
    );
    assert_eq!(count(&h, &e), 1);
    // Calling the hook outside Token-2022 cannot manufacture a settlement event.
    let mut ix = spl_transfer_hook_interface::instruction::execute(
        &HOOK,
        &e.wallet,
        &e.mint,
        &e.vault,
        &h.key(1),
        100,
    );
    ix.accounts.extend([
        AccountMeta::new_readonly(hook_metadata(e.mint), false),
        AccountMeta::new(hook_state(e.mint), false),
        AccountMeta::new_readonly(ledger::ID, false),
    ]);
    rejects_with_error(&mut h, &[0], ix, InstructionError::InvalidArgument);
}

#[test]
fn permissioned_burn_preserves_owner_consent_and_program_controlled_burns() {
    let mut h = Harness::new();
    let e = initialized(&mut h, None, true, false);
    let init = e.registration(&h);
    succeeds(&mut h, &[0], init);
    let parent = branch(&mut h, e.root, 0, true);
    let holder = h.key(2);
    let ix = e.movement(&h, (h.key(0), 1), (parent, holder), 100, true, &[]);
    succeeds(&mut h, &[0, 1], ix);
    let ix =
        token_ix::burn_checked(&TOKEN_2022, &e.wallet, &e.mint, &h.key(1), &[], 10, 6).unwrap();
    rejects(
        &mut h,
        &[0, 1],
        ix,
        token2022_current::error::TokenError::InvalidInstruction as u32,
    );
    let ix = token2022_current::extension::permissioned_burn::instruction::burn_checked(
        &TOKEN_2022,
        &e.wallet,
        &e.mint,
        &h.key(2),
        &h.key(1),
        &[],
        10,
        6,
    )
    .unwrap();
    for absent in [1, 2] {
        let mut bad = ix.clone();
        for m in &mut bad.accounts {
            if m.pubkey == h.key(absent) {
                m.is_signer = false;
            }
        }
        let other = if absent == 1 { 2 } else { 1 };
        let failed = reject_unchanged(&mut h, &[0, other], bad);
        assert_eq!(
            failed.err,
            TransactionError::InstructionError(0, InstructionError::MissingRequiredSignature)
        );
    }
    succeeds(&mut h, &[0, 1, 2], ix);
    assert_eq!(h.token(e.wallet), 890);
    assert_eq!(h.record(e.source).credit, 100);
    // A program can issue tokens and burn tokens it controls using its PDA.
    let mut h = Harness::new();
    let e = initialized(&mut h, None, true, true);
    let init = e.registration(&h);
    succeeds(&mut h, &[0], init);
    let authority = app_authority(&h);
    let parent = child(e.root, authority);
    let holder = h.key(1);
    let ix = proxy(
        &h,
        APP,
        authority,
        group(&h, e.root, authority, e.root, authority, true, &[]),
    );
    succeeds(&mut h, &[0], ix);
    let movement = |h: &Harness, deposit| {
        let mut ix = e.movement(h, (authority, 0), (parent, holder), 100, deposit, &[]);
        ix.accounts[2].pubkey = authority;
        h.indexed(proxy(h, APP, authority, ix))
    };
    let deposit = movement(&h, true);
    succeeds(&mut h, &[0], deposit);
    let withdraw = movement(&h, false);
    let burn = program_token(&h, &e, 1001, true);
    let failed = reject_many_unchanged(&mut h, &[0], vec![withdraw.clone(), burn]);
    assert_eq!(
        failed.err,
        TransactionError::InstructionError(
            1,
            InstructionError::Custom(
                token2022_current::error::TokenError::InsufficientFunds as u32
            )
        )
    );
    let burn = program_token(&h, &e, 100, true);
    run_many(&mut h, &[0], &[withdraw, burn]).unwrap();
    assert_eq!(
        (
            h.token(e.wallet),
            h.token(e.vault),
            h.record(e.source).credit
        ),
        (900, 0, 0)
    );
    let data = h.account(&e.mint).unwrap().data;
    assert_eq!(
        StateWithExtensions::<MintRecord>::unpack(&data)
            .unwrap()
            .base
            .supply,
        900
    );
}

#[test]
fn hook_configuration_changes_are_observed_on_every_custody_call() {
    let mut h = Harness::new();
    let e = initialized(&mut h, Some(false), true, false);
    let init = e.registration(&h);
    succeeds(&mut h, &[0], init);
    let parent = branch(&mut h, e.root, 0, true);
    let holder = h.key(2);
    let call = |h: &Harness, with_extras| {
        let ix = e.movement(h, (h.key(0), 1), (parent, holder), 10, true, &[]);
        if with_extras {
            extras(&e, ix)
        } else {
            ix
        }
    };
    let update = |h: &Harness, hook| {
        token2022_current::extension::transfer_hook::instruction::update(
            &TOKEN_2022,
            &e.mint,
            &h.key(0),
            &[],
            hook,
        )
        .unwrap()
    };
    let ix = call(&h, false);
    succeeds(&mut h, &[0, 1], ix);
    assert_eq!(count(&h, &e), 0);
    let ix = update(&h, Some(HOOK));
    succeeds(&mut h, &[0], ix);
    let ix = call(&h, false);
    let failed = reject_unchanged(&mut h, &[0, 1], ix);
    assert_eq!(
        failed.err,
        TransactionError::InstructionError(
            0,
            InstructionError::Custom(
                spl_transfer_hook_interface::error::TransferHookError::IncorrectAccount as u32,
            )
        )
    );
    let ix = call(&h, true);
    succeeds(&mut h, &[0, 1], ix);
    assert_eq!(count(&h, &e), 1);
    let ix = update(&h, None);
    succeeds(&mut h, &[0], ix);
    mode(&mut h, &e, 1);
    let ix = call(&h, false);
    succeeds(&mut h, &[0, 1], ix);
    assert_eq!(count(&h, &e), 1);
    let ix = update(&h, Some(HOOK));
    succeeds(&mut h, &[0], ix);
    let ix = call(&h, true);
    rejects(&mut h, &[0, 1], ix, 7101);
    assert_eq!((h.token(e.vault), h.record(e.source).credit), (30, 30));
}

#[test]
fn underbacking_still_freezes_hook_tokens_and_direct_repairs_restore_activity() {
    let mut h = Harness::new();
    let e = initialized(&mut h, Some(true), true, false);
    let init = e.registration(&h);
    succeeds(&mut h, &[0], init);
    let parent = branch(&mut h, e.root, 0, true);
    let holder = h.key(2);
    let ix = extras(
        &e,
        e.movement(&h, (h.key(0), 1), (parent, holder), 100, true, &[]),
    );
    succeeds(&mut h, &[0, 1], ix);
    let mut damaged = h.account(&e.vault).unwrap();
    damaged.data[64..72].copy_from_slice(&80u64.to_le_bytes());
    h.svm.set_account(e.vault, damaged).unwrap();
    for deposit in [true, false] {
        for amount in [0, 1] {
            let ix = extras(
                &e,
                e.movement(&h, (h.key(0), 1), (parent, holder), amount, deposit, &[]),
            );
            rejects(&mut h, &[0, 1], ix, LedgerError::Undercollateralized.into());
        }
    }
    let ix = transfer(
        &h,
        e.root,
        h.key(0),
        (parent, holder),
        (parent, h.key(1)),
        1,
        &[],
    );
    rejects(&mut h, &[0], ix, LedgerError::Undercollateralized.into());
    assert_eq!(count(&h, &e), 1);
    let repair = extras(
        &e,
        token_ix::transfer_checked(
            &TOKEN_2022,
            &e.wallet,
            &e.mint,
            &e.vault,
            &h.key(1),
            &[],
            20,
            6,
        )
        .unwrap(),
    );
    succeeds(&mut h, &[0, 1], repair);
    let ix = extras(
        &e,
        e.movement(&h, (h.key(0), 1), (parent, holder), 10, false, &[]),
    );
    succeeds(&mut h, &[0, 1], ix);
    assert_eq!((h.token(e.vault), h.record(e.source).credit), (90, 90));
}
