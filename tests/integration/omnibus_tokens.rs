use super::*;
use spl_tlv_account_resolution::{account::ExtraAccountMeta, state::ExtraAccountMetaList};
use spl_token_2022_interface::{
    self as token,
    extension::{
        immutable_owner::ImmutableOwner,
        metadata_pointer::MetadataPointer,
        permanent_delegate::PermanentDelegate,
        permissioned_burn::PermissionedBurnConfig,
        transfer_fee::TransferFeeConfig,
        transfer_hook::{TransferHook, TransferHookAccount},
        BaseStateWithExtensions, BaseStateWithExtensionsMut, ExtensionType, StateWithExtensions,
        StateWithExtensionsMut,
    },
    state::{Account as NativeAccount, Mint as NativeMint},
};

impl Harness {
    fn put_native(&mut self, key: Address, owner: Address, data: Vec<u8>) {
        self.svm
            .set_account(
                key,
                Account {
                    lamports: self.svm.minimum_balance_for_rent_exemption(data.len()),
                    data,
                    owner,
                    executable: false,
                    rent_epoch: u64::MAX,
                },
            )
            .unwrap();
    }
    fn native_asset(
        &mut self,
        extensions: &[ExtensionType],
        hook: Option<Address>,
        mutable_hook: bool,
    ) -> AssetFixture {
        let mint = self.address();
        let mut data =
            vec![0; ExtensionType::try_calculate_account_len::<NativeMint>(extensions).unwrap()];
        let mut state =
            StateWithExtensionsMut::<NativeMint>::unpack_uninitialized(&mut data).unwrap();
        for ext in extensions {
            match ext {
                ExtensionType::TransferHook => {
                    let value = state.init_extension::<TransferHook>(true).unwrap();
                    value.program_id = hook.try_into().unwrap();
                    if mutable_hook {
                        value.authority = Some(self.keys[0].pubkey()).try_into().unwrap();
                    }
                }
                ExtensionType::TransferFeeConfig => {
                    state.init_extension::<TransferFeeConfig>(true).unwrap();
                }
                ExtensionType::PermanentDelegate => {
                    state
                        .init_extension::<PermanentDelegate>(true)
                        .unwrap()
                        .delegate = Some(self.keys[0].pubkey()).try_into().unwrap();
                }
                ExtensionType::PermissionedBurn => {
                    state
                        .init_extension::<PermissionedBurnConfig>(true)
                        .unwrap()
                        .authority = Some(self.keys[0].pubkey()).try_into().unwrap();
                }
                ExtensionType::MetadataPointer => {
                    state.init_extension::<MetadataPointer>(true).unwrap();
                }
                _ => panic!("unsupported test extension"),
            }
        }
        state.base = NativeMint {
            mint_authority: COption::None,
            supply: INITIAL_TOKENS * 3,
            decimals: 6,
            is_initialized: true,
            freeze_authority: COption::None,
        };
        state.pack_base();
        state.init_account_type().unwrap();
        self.put_native(mint, TOKEN_2022, data);
        let asset = pda(&[b"asset", self.ledger.as_ref(), mint.as_ref()]);
        let vault = pda(&[b"vault", asset.as_ref()]);
        let wallets = std::array::from_fn(|i| {
            let key = self.address();
            let mut types = vec![ExtensionType::ImmutableOwner];
            if hook.is_some() {
                types.push(ExtensionType::TransferHookAccount);
            }
            let mut data =
                vec![0; ExtensionType::try_calculate_account_len::<NativeAccount>(&types).unwrap()];
            let mut state =
                StateWithExtensionsMut::<NativeAccount>::unpack_uninitialized(&mut data).unwrap();
            state.init_extension::<ImmutableOwner>(true).unwrap();
            if hook.is_some() {
                state.init_extension::<TransferHookAccount>(true).unwrap();
            }
            state.base = NativeAccount {
                mint,
                owner: self.keys[i].pubkey(),
                amount: INITIAL_TOKENS,
                delegate: COption::None,
                state: token::state::AccountState::Initialized,
                is_native: COption::None,
                delegated_amount: 0,
                close_authority: COption::None,
            };
            state.pack_base();
            state.init_account_type().unwrap();
            self.put_native(key, TOKEN_2022, data);
            key
        });
        let positions = std::array::from_fn(|i| {
            pda(&[b"position", asset.as_ref(), self.keys[i].pubkey().as_ref()])
        });
        AssetFixture {
            mint,
            asset,
            vault,
            wallets,
            positions,
        }
    }
    fn native_register(&self, a: &AssetFixture) -> Instruction {
        let mut instruction = self.register_ix(a, 0);
        instruction.accounts[5].pubkey = TOKEN_2022;
        instruction
    }
    fn register_native(&mut self, a: &AssetFixture) {
        self.run(0, &[self.native_register(a)]).unwrap();
        for i in 0..3 {
            self.run(i, &[self.open_ix(a, i)]).unwrap();
        }
    }
    fn native_move(
        &self,
        a: &AssetFixture,
        user: usize,
        amount: u64,
        deposit: bool,
    ) -> Instruction {
        let mut instruction = self.move_ix(a, user, amount, deposit);
        instruction.accounts[6].pubkey = TOKEN_2022;
        instruction
    }
    fn native_amount(&self, key: Address) -> u64 {
        StateWithExtensions::<NativeAccount>::unpack(&self.svm.get_account(&key).unwrap().data)
            .unwrap()
            .base
            .amount
    }
}

#[test]
fn native_custody_keeps_claims_separate_and_preserves_supply() {
    for extensions in [
        vec![],
        vec![ExtensionType::MetadataPointer],
        vec![ExtensionType::PermissionedBurn],
    ] {
        let mut h = Harness::new();
        let a = h.native_asset(&extensions, None, false);
        h.register_native(&a);
        let mint_before = h.svm.get_account(&a.mint);
        h.run(1, &[h.native_move(&a, 1, 100, true)]).unwrap();
        let vault_before = h.svm.get_account(&a.vault);
        h.run(1, &[h.transfer_ix(&a, 1, 2, 40)]).unwrap();
        assert_eq!(h.svm.get_account(&a.vault), vault_before);
        h.run(2, &[h.native_move(&a, 2, 40, false)]).unwrap();
        assert_eq!(h.native_amount(a.vault), 60);
        assert_eq!(h.state::<Asset>(a.asset).total_claims, 60);
        assert_eq!(h.state::<Position>(a.positions[1]).balance, 60);
        assert_eq!(h.state::<Position>(a.positions[2]).balance, 0);
        assert_eq!(h.native_amount(a.wallets[2]), INITIAL_TOKENS + 40);
        assert_eq!(h.svm.get_account(&a.mint), mint_before);
        let data = h.svm.get_account(&a.vault).unwrap().data;
        let vault = StateWithExtensions::<NativeAccount>::unpack(&data).unwrap();
        assert!(vault.get_extension::<ImmutableOwner>().is_ok());
        // Permissioned-burn authority alone cannot burn collateral it does not own.
        if extensions.contains(&ExtensionType::PermissionedBurn) {
            let before = h.snapshot(&a);
            let mut burn = token::extension::permissioned_burn::instruction::burn_checked(
                &TOKEN_2022,
                &a.vault,
                &a.mint,
                &h.keys[0].pubkey(),
                &a.asset,
                &[],
                1,
                6,
            )
            .unwrap();
            burn.accounts[3].is_signer = false;
            assert!(h.run(0, &[burn]).is_err());
            assert_eq!(h.snapshot(&a), before);
        }
    }
}

#[test]
fn rejects_balance_changing_extensions_and_mutable_hooks_atomically() {
    for extension in [
        ExtensionType::TransferFeeConfig,
        ExtensionType::PermanentDelegate,
        ExtensionType::TransferHook,
    ] {
        let mut h = Harness::new();
        let hook = h.address();
        let a = h.native_asset(&[extension], Some(hook), true);
        assert!(h.run(0, &[h.native_register(&a)]).is_err());
        assert!(h.svm.get_account(&a.asset).is_none());
        assert!(h.svm.get_account(&a.vault).is_none());
    }
}

#[test]
fn transfer_hook_executes_only_at_custody_boundary_and_failure_rolls_back() {
    let mut h = Harness::new();
    let hook = h.address();
    let bytes = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledgers_consumer_test.so"),
    )
    .unwrap();
    h.svm.add_program(hook, &bytes).unwrap();
    let a = h.native_asset(
        &[ExtensionType::TransferHook, ExtensionType::PermissionedBurn],
        Some(hook),
        false,
    );
    let meta = spl_transfer_hook_interface::get_extra_account_metas_address(&a.mint, &hook);
    let counter = h.address();
    h.put_native(counter, hook, vec![0; 8]);
    let mut data = vec![0; ExtraAccountMetaList::size_of(1).unwrap()];
    ExtraAccountMetaList::init::<spl_transfer_hook_interface::instruction::ExecuteInstruction>(
        &mut data,
        &[ExtraAccountMeta::new_with_pubkey(&counter, false, true).unwrap()],
    )
    .unwrap();
    h.put_native(meta, hook, data);
    h.register_native(&a);
    let movement = |h: &Harness, amount, deposit| {
        let mut instruction = h.native_move(&a, 1, amount, deposit);
        instruction.accounts.extend([
            AccountMeta::new_readonly(hook, false),
            AccountMeta::new_readonly(meta, false),
            AccountMeta::new(counter, false),
        ]);
        instruction
    };
    let before = h.snapshot(&a);
    assert!(h.run(1, &[h.native_move(&a, 1, 100, true)]).is_err());
    assert_eq!(h.snapshot(&a), before);
    h.run(1, &[movement(&h, 100, true)]).unwrap();
    assert_eq!(
        h.svm.get_account(&counter).unwrap().data,
        1u64.to_le_bytes()
    );
    h.run(1, &[h.transfer_ix(&a, 1, 2, 10)]).unwrap();
    assert_eq!(
        h.svm.get_account(&counter).unwrap().data,
        1u64.to_le_bytes()
    );
    for deposit in [true, false] {
        let before = h.snapshot(&a);
        assert!(h.run(1, &[movement(&h, 13, deposit)]).is_err());
        assert_eq!(h.snapshot(&a), before);
        assert_eq!(
            h.svm.get_account(&counter).unwrap().data,
            1u64.to_le_bytes()
        );
    }
    h.run(1, &[movement(&h, 40, false)]).unwrap();
    assert_eq!(
        h.svm.get_account(&counter).unwrap().data,
        2u64.to_le_bytes()
    );
    assert_eq!(h.native_amount(a.vault), 60);
    assert_eq!(h.state::<Asset>(a.asset).total_claims, 60);
}
