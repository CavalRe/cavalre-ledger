//! Native token boundary. Claims always use exact base units.
use crate::{CustodyError as E, MoveTokens, RegisterAsset};
use anchor_lang::{
    prelude::*,
    solana_program::{
        program::{invoke, invoke_signed},
        program_pack::Pack,
    },
};
use spl_token_2022_interface::{
    self as token,
    extension::{
        transfer_hook::TransferHook, BaseStateWithExtensions, ExtensionType, StateWithExtensions,
    },
    state::{Account, Mint},
};

/// Accept balance-preserving extensions only. Recheck at every custody boundary;
/// registering a mint does not make a mutable extension configuration trusted.
pub(crate) fn validate_mint(mint: &AccountInfo) -> Result<Option<Pubkey>> {
    let data = mint.try_borrow_data()?;
    let state = StateWithExtensions::<Mint>::unpack(&data)?;
    let mut hook = None;
    for extension in state.get_extension_types()? {
        match extension {
            ExtensionType::MetadataPointer
            | ExtensionType::TokenMetadata
            | ExtensionType::GroupPointer
            | ExtensionType::TokenGroup
            | ExtensionType::GroupMemberPointer
            | ExtensionType::TokenGroupMember
            // This adds a required burn cosigner; it does not replace the
            // holder's signature. Ledger never signs an external vault burn.
            | ExtensionType::PermissionedBurn => {}
            ExtensionType::TransferHook => {
                let config = state.get_extension::<TransferHook>()?;
                require!(
                    Option::<Pubkey>::from(config.authority).is_none(),
                    E::UnsupportedTokenExtension
                );
                hook = config.program_id.into();
            }
            _ => return err!(E::UnsupportedTokenExtension),
        }
    }
    Ok(hook)
}

pub(crate) fn vault_space(mint: &AccountInfo, token_program: Pubkey) -> Result<usize> {
    require_keys_eq!(*mint.owner, token_program, E::InvalidNode);
    validate_mint(mint)?;
    if token_program != token::ID {
        return Ok(Account::LEN);
    }
    let data = mint.try_borrow_data()?;
    Ok(
        token::extension::account_len::try_calculate_account_len_from_mint_data(
            &data,
            &[ExtensionType::ImmutableOwner],
        )?,
    )
}

pub(crate) fn initialize_vault(accounts: &RegisterAsset) -> Result<()> {
    let vault = accounts.vault.to_account_info();
    if accounts.token_program.key() == token::ID {
        invoke(
            &token::instruction::initialize_immutable_owner(&token::ID, vault.key)?,
            std::slice::from_ref(&vault),
        )?;
    }
    invoke(
        &token::instruction::initialize_account3(
            &accounts.token_program.key(),
            vault.key,
            &accounts.mint.key(),
            &accounts.asset.key(),
        )?,
        &[vault, accounts.mint.to_account_info()],
    )
    .map_err(Into::into)
}

pub(crate) fn transfer<'info>(
    accounts: &MoveTokens<'info>,
    remaining: &[AccountInfo<'info>],
    amount: u64,
    deposit: bool,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let mint = accounts.mint.to_account_info();
    let hook = validate_mint(&mint)?;
    let vault = accounts.vault.to_account_info();
    let wallet = accounts.wallet.to_account_info();
    // A vault cannot acquire another spending or close authority through this
    // service. Unknown account extensions also require explicit review.
    require!(
        accounts.vault.delegate.is_none() && accounts.vault.close_authority.is_none(),
        E::InvalidNode
    );
    for info in [&vault, &wallet] {
        let data = info.try_borrow_data()?;
        let state = StateWithExtensions::<Account>::unpack(&data)?;
        require!(
            state
                .get_extension_types()?
                .iter()
                .all(|extension| matches!(
                    extension,
                    ExtensionType::ImmutableOwner | ExtensionType::TransferHookAccount
                )),
            E::UnsupportedTokenExtension
        );
    }
    let (from, to, authority) = if deposit {
        (wallet, vault, accounts.owner.to_account_info())
    } else {
        (vault, wallet, accounts.asset.to_account_info())
    };
    let mut instruction = token::instruction::transfer_checked(
        &accounts.token_program.key(),
        from.key,
        mint.key,
        to.key,
        authority.key,
        &[],
        amount,
        accounts.mint.decimals,
    )?;
    let mut infos = vec![from.clone(), mint.clone(), to.clone(), authority.clone()];
    if let Some(program) = hook {
        spl_transfer_hook_interface::onchain::add_extra_accounts_for_execute_cpi(
            &mut instruction,
            &mut infos,
            &program,
            from,
            mint,
            to,
            authority,
            amount,
            remaining,
        )?;
    } else {
        require!(remaining.is_empty(), E::InvalidAccounts);
    }
    invoke_signed(&instruction, &infos, signer_seeds).map_err(Into::into)
}
