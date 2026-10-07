//! Explicit packed encoding. Rust's in-memory alignment never defines storage.
use crate::{err, IndexedError};
use anchor_lang::prelude::*;
use cavalre_ledger_core::ledger_lib::{AccountKind, Balances, Flags, TokenKind};

pub const HEADER_LEN: usize = 80;
pub const RECORD_LEN: usize = 110;
pub const MAGIC: &[u8; 8] = b"CVIDX002";
pub const TOMBSTONE: u8 = 255;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccountRef {
    pub container: Pubkey,
    pub index: u32,
}
impl AccountRef {
    pub fn new(container: Pubkey, index: u32) -> Self {
        Self { container, index }
    }
    fn read(data: &[u8], at: usize) -> Self {
        Self {
            container: Pubkey::new_from_array(data[at..at + 32].try_into().unwrap()),
            index: read_u32(data, at + 32),
        }
    }
    fn write(self, data: &mut [u8], at: usize) {
        data[at..at + 32].copy_from_slice(self.container.as_ref());
        data[at + 32..at + 36].copy_from_slice(&self.index.to_le_bytes());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub custodian: AccountRef,
    pub parent: AccountRef,
    pub depth: u8,
    pub kind: u8,
    pub debit: u128,
    pub credit: u128,
    pub child_count: u32,
}
impl Record {
    pub fn balances(self) -> Balances {
        Balances {
            debit: self.debit,
            credit: self.credit,
        }
    }
    pub fn flags(self) -> Result<Flags<AccountRef>> {
        Ok(Flags {
            parent: self.parent,
            depth: self.depth,
            account_kind: kind(self.kind)?,
            token_kind: if self.depth == 2 {
                TokenKind::Internal
            } else {
                TokenKind::Unregistered
            },
        })
    }
}
pub fn kind(value: u8) -> Result<AccountKind> {
    Ok(match value {
        0 => AccountKind::DebitGroup,
        1 => AccountKind::CreditGroup,
        2 => AccountKind::DebitLedger,
        3 => AccountKind::CreditLedger,
        _ => return Err(err(IndexedError::InvalidRecord)),
    })
}
fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}
fn read_u128(data: &[u8], at: usize) -> u128 {
    u128::from_le_bytes(data[at..at + 16].try_into().unwrap())
}

#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub authority: Pubkey,
    pub ledger: AccountRef,
    pub len: u32,
}
impl Header {
    pub fn read(data: &[u8]) -> Result<Self> {
        require!(
            data.len() >= HEADER_LEN && &data[..8] == MAGIC,
            IndexedError::InvalidContainer
        );
        require!(
            (data.len() - HEADER_LEN).is_multiple_of(RECORD_LEN),
            IndexedError::InvalidContainer
        );
        let header = Self {
            authority: Pubkey::new_from_array(data[8..40].try_into().unwrap()),
            ledger: AccountRef::read(data, 40),
            len: read_u32(data, 76),
        };
        require!(
            header.len as usize <= (data.len() - HEADER_LEN) / RECORD_LEN,
            IndexedError::InvalidContainer
        );
        Ok(header)
    }
    pub fn write(self, data: &mut [u8]) {
        data[..8].copy_from_slice(MAGIC);
        data[8..40].copy_from_slice(self.authority.as_ref());
        self.ledger.write(data, 40);
        data[76..80].copy_from_slice(&self.len.to_le_bytes());
    }
}
fn offset(data: &[u8], index: u32) -> Result<usize> {
    let header = Header::read(data)?;
    require!(index < header.len, IndexedError::InvalidRecord);
    Ok(HEADER_LEN + index as usize * RECORD_LEN)
}
pub fn read_record(data: &[u8], index: u32) -> Result<Record> {
    let at = offset(data, index)?;
    let record = Record {
        custodian: AccountRef::read(data, at),
        parent: AccountRef::read(data, at + 36),
        depth: data[at + 72],
        kind: data[at + 73],
        debit: read_u128(data, at + 74),
        credit: read_u128(data, at + 90),
        child_count: read_u32(data, at + 106),
    };
    kind(record.kind)?;
    require!(
        record.depth >= 2 && (record.kind < 2 || record.child_count == 0),
        IndexedError::InvalidRecord
    );
    Ok(record)
}
pub fn write_record(data: &mut [u8], index: u32, record: Record) -> Result<()> {
    let at = offset(data, index)?;
    record.custodian.write(data, at);
    record.parent.write(data, at + 36);
    data[at + 72] = record.depth;
    data[at + 73] = record.kind;
    data[at + 74..at + 90].copy_from_slice(&record.debit.to_le_bytes());
    data[at + 90..at + 106].copy_from_slice(&record.credit.to_le_bytes());
    data[at + 106..at + 110].copy_from_slice(&record.child_count.to_le_bytes());
    Ok(())
}
pub fn append(data: &mut [u8], record: Record) -> Result<u32> {
    let mut header = Header::read(data)?;
    require!(
        (header.len as usize) < (data.len() - HEADER_LEN) / RECORD_LEN,
        IndexedError::Full
    );
    let index = header.len;
    header.len = header.len.checked_add(1).ok_or(err(IndexedError::Full))?;
    header.write(data);
    write_record(data, index, record)?;
    Ok(index)
}
pub fn set_balances(data: &mut [u8], index: u32, balance: Balances) -> Result<()> {
    let at = offset(data, index)?;
    data[at + 74..at + 90].copy_from_slice(&balance.debit.to_le_bytes());
    data[at + 90..at + 106].copy_from_slice(&balance.credit.to_le_bytes());
    Ok(())
}
pub fn set_child_count(data: &mut [u8], index: u32, count: u32) -> Result<()> {
    let at = offset(data, index)?;
    data[at + 106..at + 110].copy_from_slice(&count.to_le_bytes());
    Ok(())
}
pub fn remove(data: &mut [u8], index: u32) -> Result<()> {
    let at = offset(data, index)?;
    data[at..at + RECORD_LEN].fill(0);
    data[at + 73] = TOMBSTONE;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_layout_and_stable_slots() {
        let reference = AccountRef::new(Pubkey::new_from_array([11; 32]), 7);
        let mut data = vec![0; HEADER_LEN + 3 * RECORD_LEN];
        Header {
            authority: reference.container,
            ledger: reference,
            len: 0,
        }
        .write(&mut data);
        let record = Record {
            custodian: reference,
            parent: reference,
            depth: 4,
            kind: 2,
            debit: u128::MAX,
            credit: 0,
            child_count: 0,
        };
        assert_eq!(append(&mut data, record).unwrap(), 0);
        assert_eq!(
            &data[HEADER_LEN..HEADER_LEN + 32],
            reference.container.as_ref()
        );
        assert_eq!(&data[HEADER_LEN + 32..HEADER_LEN + 36], &7u32.to_le_bytes());
        assert_eq!(&data[HEADER_LEN + 72..HEADER_LEN + 74], &[4, 2]);
        assert_eq!(&data[HEADER_LEN + 74..HEADER_LEN + 90], &[255; 16]);
        assert_eq!(read_record(&data, 0).unwrap(), record);
        append(&mut data, record).unwrap();
        remove(&mut data, 0).unwrap();
        assert!(read_record(&data, 0).is_err());
        assert_eq!(append(&mut data, record).unwrap(), 2);
        assert_eq!(read_record(&data, 1).unwrap(), record);
        assert!(append(&mut data, record).is_err());
        for size in [0, 79, HEADER_LEN + 1] {
            assert!(Header::read(&data[..size]).is_err());
        }
        assert!(read_record(&data, u32::MAX).is_err());
    }
}
