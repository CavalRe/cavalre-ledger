//! In-place custody instruction preparation for clients and CPI callers.
use anchor_lang::{
    solana_program::{instruction::Instruction, program_error::ProgramError},
    Discriminator,
};

/// Set only the ledger meta of an encoded wrap/unwrap instruction from its amount.
///
/// Supports classic SPL, Token-2022 and native SOL. Checks the instruction kind
/// and fixed layout without decoding/re-encoding arguments or allocating memory.
/// Data, signer flags and all other account metas are preserved. Call before
/// signing a transaction or using `invoke_signed` with the caller's account infos
/// and signer seeds. The outer transaction must supply every required privilege.
pub fn set_custody_ledger_writable(instruction: &mut Instruction) -> Result<(), ProgramError> {
    use crate::instruction::{Unwrap, UnwrapSol, Wrap, WrapSol};

    // All four layouts encode discriminator, parent, relative, amount (u64 LE).
    let data = &instruction.data;
    if data.len() != 8 + 32 + 32 + 8 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let account_count = if data.starts_with(Wrap::DISCRIMINATOR)
        || data.starts_with(Unwrap::DISCRIMINATOR)
    {
        9
    } else if data.starts_with(WrapSol::DISCRIMINATOR) || data.starts_with(UnwrapSol::DISCRIMINATOR)
    {
        7
    } else {
        return Err(ProgramError::InvalidInstructionData);
    };
    if instruction.accounts.len() < account_count {
        return Err(ProgramError::NotEnoughAccountKeys);
    }
    // Ledger follows payer, authority and funder in both custody account layouts.
    // Touch only this role, including when another role shares the ledger's key.
    instruction.accounts[3].is_writable = data[72..80] != [0; 8];
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::{prelude::Pubkey, solana_program::instruction::AccountMeta, InstructionData};

    fn instructions(amount: u64) -> [(Instruction, usize); 4] {
        use crate::instruction::{Unwrap, UnwrapSol, Wrap, WrapSol};
        let parent = Pubkey::new_from_array([1; 32]);
        let relative = Pubkey::new_from_array([2; 32]);
        [
            (
                Wrap {
                    parent,
                    relative,
                    amount,
                }
                .data(),
                9,
            ),
            (
                Unwrap {
                    parent,
                    relative,
                    amount,
                }
                .data(),
                9,
            ),
            (
                WrapSol {
                    parent,
                    relative,
                    amount,
                }
                .data(),
                7,
            ),
            (
                UnwrapSol {
                    parent,
                    relative,
                    amount,
                }
                .data(),
                7,
            ),
        ]
        .map(|(data, fixed)| {
            (
                Instruction {
                    program_id: crate::ID,
                    data,
                    // Include remaining records and duplicate identities with distinct
                    // role permissions, so only the ledger meta may change.
                    accounts: (0..fixed + 2)
                        .map(|i| AccountMeta {
                            pubkey: parent,
                            is_signer: i % 2 == 0,
                            is_writable: i % 2 == 1,
                        })
                        .collect(),
                },
                fixed,
            )
        })
    }

    #[test]
    fn custody_preparation_preserves_encoded_data_and_every_other_meta() {
        for amount in [0, 1, 256, 1 << 63, u64::MAX] {
            for (mut instruction, _) in instructions(amount) {
                instruction.accounts[3].is_writable = amount == 0;
                let mut expected = instruction.clone();
                expected.accounts[3].is_writable = amount != 0;
                let pointers = (instruction.data.as_ptr(), instruction.accounts.as_ptr());
                for _ in 0..2 {
                    set_custody_ledger_writable(&mut instruction).unwrap();
                    assert_eq!(instruction, expected);
                    assert_eq!(
                        (instruction.data.as_ptr(), instruction.accounts.as_ptr()),
                        pointers
                    );
                }
            }
        }
    }

    #[test]
    fn malformed_custody_instructions_reject_without_modification() {
        for (instruction, fixed) in instructions(1) {
            let mut short_data = instruction.clone();
            short_data.data.pop();
            let mut extra_data = instruction.clone();
            extra_data.data.push(0);
            let mut wrong_kind = instruction.clone();
            wrong_kind.data[..8].fill(0);
            let mut short_accounts = instruction;
            short_accounts.accounts.truncate(fixed - 1);
            for (mut invalid, error) in [
                (short_data, ProgramError::InvalidInstructionData),
                (extra_data, ProgramError::InvalidInstructionData),
                (wrong_kind, ProgramError::InvalidInstructionData),
                (short_accounts, ProgramError::NotEnoughAccountKeys),
            ] {
                let before = invalid.clone();
                assert_eq!(set_custody_ledger_writable(&mut invalid), Err(error));
                assert_eq!(invalid, before);
            }
        }
    }
}
