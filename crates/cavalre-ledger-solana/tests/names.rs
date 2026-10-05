use anchor_lang::prelude::Pubkey;
use cavalre_ledger_core::ledger_lib as core;
use cavalre_ledger_solana::{ledger_lib::*, ID};

#[test]
fn name_identity_matches_keccak_and_explicit_pda_derivation() {
    // Published Keccak-256("abc") vector, not SHA3-256 or an EVM truncation.
    let hex = "4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45";
    let expected = Pubkey::new_from_array(std::array::from_fn(|i| {
        u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()
    }));
    assert_eq!(name_to_address("abc"), Ok(expected));
    let parent = Pubkey::new_from_array([7; 32]);
    let named = to_address_by_name(&ID, &parent, "abc").unwrap();
    assert_eq!(named, to_address(&ID, &parent, &expected));
    assert_eq!(
        core::to_address_by_name(&PdaAddresses(&ID), &parent, "abc"),
        Ok(named.0)
    );
    assert_ne!(
        named.0,
        to_address_by_name(&ID, &Pubkey::new_from_array([8; 32]), "abc")
            .unwrap()
            .0
    );
    assert_ne!(
        named.0,
        to_address_by_name(&parent, &parent, "abc").unwrap().0
    );
    assert_eq!(name_to_address("Source").unwrap(), SOURCE);
    assert_eq!(
        to_address_by_name(&ID, &parent, "Source").unwrap(),
        to_address(&ID, &parent, &SOURCE)
    );
    // Original Solidity SOURCE_ADDRESS is the low 20 bytes of this same hash.
    assert_eq!(
        &SOURCE.to_bytes()[12..],
        &[
            0x24, 0x5f, 0x14, 0xe6, 0x1e, 0xcd, 0xe5, 0x91, 0xfd, 0x8b, 0x44, 0x5d, 0xc8, 0xe2,
            0xbf, 0x76, 0xda, 0x45, 0x05, 0xe6
        ]
    );
}

#[test]
fn named_identities_validate_exact_utf8_bytes_without_normalization() {
    for name in ["".to_owned(), "x".repeat(65), "é".repeat(33)] {
        assert_eq!(name_to_address(&name), Err(Error::InvalidName));
        assert_eq!(to_address_by_name(&ID, &ID, &name), Err(Error::InvalidName));
    }
    for name in ["x".repeat(64), "é".repeat(32), " ".into()] {
        assert!(name_to_address(&name).is_ok());
    }
    for (a, b) in [
        ("Rewards", "rewards"),
        ("Rewards", "Rewards "),
        ("é", "e\u{301}"),
    ] {
        assert_ne!(name_to_address(a).unwrap(), name_to_address(b).unwrap());
    }
}
