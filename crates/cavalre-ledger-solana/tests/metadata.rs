//! Asset reads also run with the mutating module and Anchor SPL excluded.
use anchor_lang::{prelude::*, solana_program::program_pack::Pack};
use cavalre_ledger_core::ledger_lib::Error as CoreError;
use cavalre_ledger_solana::{
    ledger_lib::{root_address, Record, NATIVE_SOL},
    ledger_view::{
        metadata_address, native_decimals, native_symbol, Reader, METAPLEX_METADATA_PROGRAM,
    },
    ID,
};
use spl_token_2022_interface::{
    extension::{
        metadata_pointer::MetadataPointer, BaseStateWithExtensionsMut, ExtensionType,
        StateWithExtensionsMut,
    },
    state::Mint,
};
use spl_token_metadata_interface::state::TokenMetadata;

fn key(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}
fn root(scope: Pubkey, mint: Pubkey) -> (Pubkey, Vec<u8>) {
    let (address, bump) = root_address(&scope, &mint);
    let record = Record {
        root: address,
        parent: cavalre_ledger_solana::ledger_lib::global_root_address().0,
        relative: mint,
        custodian: Pubkey::default(),
        kind: 0,
        token_kind: if scope != Pubkey::default() {
            3
        } else if mint == NATIVE_SOL {
            1
        } else {
            2
        },
        depth: 2,
        registered: true,
        implicit_allowed: true,
        children: 1,
        debit: 0,
        credit: 0,
        name: "Ledger name".into(),
        scope,
        identifier: mint,
        bump,
        sub_index: 1,
        symbol: if mint == NATIVE_SOL {
            "SOL".into()
        } else {
            "CACHED".into()
        },
        decimals: if mint == NATIVE_SOL { 9 } else { 3 },
    };
    let mut data = vec![0; 512];
    data[..8].copy_from_slice(b"CVLEDG01");
    record.serialize(&mut &mut data[8..]).unwrap();
    (address, data)
}
fn reader(mint: Pubkey) -> (Reader, Pubkey) {
    let (root, data) = root(Pubkey::default(), mint);
    let mut reader = Reader::new();
    reader.insert(root, &ID, &data).unwrap();
    (reader, root)
}
fn plain_mint(decimals: u8) -> Vec<u8> {
    let mut data = vec![0; Mint::LEN];
    Mint::pack(
        Mint {
            decimals,
            is_initialized: true,
            ..Mint::default()
        },
        &mut data,
    )
    .unwrap();
    data
}
fn extended_mint(mint: Pubkey, target: Option<Pubkey>, inline: bool) -> Vec<u8> {
    let metadata = TokenMetadata {
        mint,
        name: "Token name".into(),
        symbol: "T22".into(),
        ..TokenMetadata::default()
    };
    let size = ExtensionType::try_calculate_account_len::<Mint>(&[ExtensionType::MetadataPointer])
        .unwrap()
        + if inline {
            // SPL interface TLV header is 12 bytes; a mint extension uses 4.
            metadata.tlv_size_of().unwrap() - 8
        } else {
            0
        };
    let mut data = vec![0; size];
    let mut state = StateWithExtensionsMut::<Mint>::unpack_uninitialized(&mut data).unwrap();
    state
        .init_extension::<MetadataPointer>(true)
        .unwrap()
        .metadata_address = target.try_into().unwrap();
    if inline {
        state.init_variable_len_extension(&metadata, false).unwrap();
    }
    state.base = Mint {
        decimals: 6,
        is_initialized: true,
        ..Mint::default()
    };
    state.pack_base();
    state.init_account_type().unwrap();
    data
}
fn metaplex(mint: Pubkey) -> Vec<u8> {
    // MetadataV1's published Borsh layout, including its required trailing fields.
    let mut bytes = vec![4];
    bytes.extend_from_slice(key(9).as_ref());
    bytes.extend_from_slice(mint.as_ref());
    "Token name\0\0".serialize(&mut bytes).unwrap();
    "SPL\0\0\0\0\0\0\0".serialize(&mut bytes).unwrap();
    "https://example.invalid/token"
        .serialize(&mut bytes)
        .unwrap();
    0u16.serialize(&mut bytes).unwrap();
    bytes.extend_from_slice(&[0; 9]); // no creators/options; false sale/mutability
    bytes
}

#[test]
fn native_internal_and_external_queries_read_stored_metadata() {
    let (reader, native) = reader(NATIVE_SOL);
    assert_eq!(native_symbol(), "SOL");
    assert_eq!(native_decimals(), 9);
    assert_eq!(reader.symbol(&native), Ok(Some("SOL".into())));
    assert_eq!(reader.decimals(&native), Ok(Some(9)));
    let (internal, data) = root(key(1), key(2));
    let mut reader = Reader::new();
    reader.insert(internal, &ID, &data).unwrap();
    assert_eq!(reader.symbol(&internal), Ok(Some("CACHED".into())));
    assert_eq!(reader.decimals(&internal), Ok(Some(3)));
    assert_eq!(reader.name(&internal), Ok("Ledger name".into()));
    assert_eq!(reader.symbol(&key(8)), Err(CoreError::MissingAccount));
    reader.insert_missing(key(8)).unwrap();
    assert_eq!(reader.decimals(&key(8)), Err(CoreError::InvalidAccount));
}

#[test]
fn classic_metadata_checks_canonical_source_and_distinguishes_missing_from_absent() {
    let mint = key(2);
    for decimals in [0, 6, 9, 255] {
        let mut reader = Reader::new();
        assert_eq!(
            reader.external_metadata(&mint),
            Err(CoreError::MissingAccount)
        );
        reader
            .insert(
                mint,
                &spl_token_2022_interface::inline_spl_token::ID,
                &plain_mint(decimals),
            )
            .unwrap();
        assert_eq!(
            reader.external_metadata(&mint),
            Err(CoreError::MissingAccount)
        );
        reader
            .insert(
                metadata_address(&mint),
                &METAPLEX_METADATA_PROGRAM,
                &metaplex(mint),
            )
            .unwrap();
        assert_eq!(
            reader.external_metadata(&mint),
            Ok(("Token name".into(), "SPL".into(), decimals))
        );
    }
    let mut reader = Reader::new();
    reader
        .insert(
            mint,
            &spl_token_2022_interface::inline_spl_token::ID,
            &plain_mint(6),
        )
        .unwrap();
    reader.insert_missing(metadata_address(&mint)).unwrap();
    assert_eq!(
        reader.external_metadata(&mint),
        Err(CoreError::InvalidMetadata)
    );
}

#[test]
fn token2022_pointer_selects_metadata_and_does_not_fall_back_to_stale_inline_fields() {
    let mint = key(2);
    for (target, inline, expected) in [
        (Some(mint), true, Ok(("Token name".into(), "T22".into(), 6))),
        (Some(mint), false, Err(CoreError::InvalidMetadata)),
        (None, true, Err(CoreError::InvalidMetadata)),
        (
            Some(metadata_address(&mint)),
            true,
            Ok(("Token name".into(), "SPL".into(), 6)),
        ),
        (Some(key(3)), true, Err(CoreError::UnsupportedMetadata)),
    ] {
        let mut reader = Reader::new();
        reader
            .insert(
                mint,
                &spl_token_2022_interface::ID,
                &extended_mint(mint, target, inline),
            )
            .unwrap();
        reader
            .insert(
                metadata_address(&mint),
                &METAPLEX_METADATA_PROGRAM,
                &metaplex(mint),
            )
            .unwrap();
        assert_eq!(reader.external_metadata(&mint), expected);
    }
    let mut reader = Reader::new();
    reader
        .insert(mint, &spl_token_2022_interface::ID, &plain_mint(0))
        .unwrap();
    reader
        .insert(
            metadata_address(&mint),
            &METAPLEX_METADATA_PROGRAM,
            &metaplex(mint),
        )
        .unwrap();
    assert_eq!(
        reader.external_metadata(&mint),
        Ok(("Token name".into(), "SPL".into(), 0))
    );
}

#[test]
fn metadata_rejects_spoofed_sources_mints_discriminators_and_truncated_strings() {
    let mint = key(2);
    let metadata = metadata_address(&mint);
    let mut reader = Reader::new();
    for (address, owner, data) in [
        (mint, key(7), plain_mint(6)),
        (mint, spl_token_2022_interface::ID, vec![0; Mint::LEN]),
        (
            mint,
            spl_token_2022_interface::inline_spl_token::ID,
            extended_mint(mint, Some(mint), true),
        ),
        (metadata, key(7), metaplex(mint)),
        (key(7), METAPLEX_METADATA_PROGRAM, metaplex(mint)),
        (metadata, METAPLEX_METADATA_PROGRAM, metaplex(key(7))),
    ] {
        assert!(reader.insert(address, &owner, &data).is_err());
    }
    let bytes = metaplex(mint);
    let symbol_end = 65 + 4 + "Token name\0\0".len() + 4 + 10;
    for end in 0..symbol_end {
        assert!(reader
            .insert(metadata, &METAPLEX_METADATA_PROGRAM, &bytes[..end])
            .is_err());
    }
    let mut corrupt = bytes.clone();
    corrupt[0] = 1;
    assert!(reader
        .insert(metadata, &METAPLEX_METADATA_PROGRAM, &corrupt)
        .is_err());
    corrupt = bytes.clone();
    corrupt[65..69].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(reader
        .insert(metadata, &METAPLEX_METADATA_PROGRAM, &corrupt)
        .is_err());
    corrupt = bytes;
    corrupt[symbol_end - 1] = 255;
    assert!(reader
        .insert(metadata, &METAPLEX_METADATA_PROGRAM, &corrupt)
        .is_err());
    reader
        .insert(metadata, &METAPLEX_METADATA_PROGRAM, &metaplex(mint))
        .unwrap();
    assert!(reader.insert_missing(metadata).is_err());
}

#[test]
fn malformed_issuer_metadata_cannot_change_stored_metadata_or_balances() {
    use spl_token_2022_interface::extension::BaseStateWithExtensions;
    let mint = key(2);
    for wrong_mint in [false, true] {
        let mut data = extended_mint(mint, Some(mint), true);
        let state = spl_token_2022_interface::extension::StateWithExtensions::<Mint>::unpack(&data)
            .unwrap();
        let payload = state.get_extension_bytes::<TokenMetadata>().unwrap();
        let offset = payload.as_ptr() as usize - data.as_ptr() as usize;
        if wrong_mint {
            data[offset + 32..offset + 64].copy_from_slice(key(3).as_ref());
        } else {
            data[offset + 64..offset + 68].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        let (mut reader, root) = reader(mint);
        reader
            .insert(mint, &spl_token_2022_interface::ID, &data)
            .unwrap();
        assert_eq!(
            reader.external_metadata(&mint),
            Err(CoreError::InvalidMetadata)
        );
        assert_eq!(reader.symbol(&root), Ok(Some("CACHED".into())));
        assert_eq!(reader.decimals(&root), Ok(Some(3)));
        assert_eq!(reader.total_supply(&root), Ok(0));
    }
    // A real mint for another asset cannot fill the selected root's missing mint.
    let (mut reader, root) = reader(mint);
    reader
        .insert(key(3), &spl_token_2022_interface::ID, &plain_mint(6))
        .unwrap();
    assert_eq!(
        reader.external_metadata(&mint),
        Err(CoreError::MissingAccount)
    );
    assert_eq!(reader.decimals(&root), Ok(Some(3)));
}

#[test]
fn runtime_account_infos_read_metadata_with_no_signers_writes_or_mutation_module() {
    let mint = key(2);
    let (root, root_data) = root(Pubkey::default(), mint);
    let mut entries = [
        (root, ID, root_data, 10_000_000),
        (
            mint,
            spl_token_2022_interface::ID,
            extended_mint(mint, Some(mint), true),
            10_000_000,
        ),
    ];
    let before = entries.clone();
    let infos: Vec<_> = entries
        .iter_mut()
        .map(|(key, owner, data, lamports)| {
            AccountInfo::new(key, false, false, lamports, data, owner, false)
        })
        .collect();
    let reader = Reader::from_account_infos(&infos).unwrap();
    assert_eq!(reader.symbol(&root), Ok(Some("CACHED".into())));
    assert_eq!(reader.decimals(&root), Ok(Some(3)));
    drop(infos);
    assert_eq!(entries, before);
}
