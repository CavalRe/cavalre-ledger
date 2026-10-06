//! Group-local mapping storage. Logical keys are independent of Solana addresses.
//! Only requested entries are decoded on the mutation path.
use crate::ledger_lib::{LedgerError, Record, MAGIC, SPACE};
use anchor_lang::prelude::*;

const MAP_MAGIC: &[u8; 8] = b"CVMAP001";
const REFERENCE: &[u8; 8] = b"CVREF001";
pub const HEADER: usize = SPACE + 16;
const VALUE: usize = 224;
const ENTRY: usize = 32 + VALUE;
const SLOT: usize = ENTRY + 32;
pub const INITIAL_CAPACITY: usize = 0;
pub const INITIAL_SPACE: usize = HEADER + INITIAL_CAPACITY * SLOT;

// Inline return avoids a separate heap allocation for every decoded leaf.
#[allow(clippy::large_enum_variant)]
pub enum Value {
    Leaf(Record),
    Group(Pubkey),
}

fn u32_at(data: &[u8], at: usize) -> usize {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize
}
pub fn capacity(data: &[u8]) -> Result<usize> {
    require!(
        data.len() >= HEADER && &data[SPACE..SPACE + 8] == MAP_MAGIC,
        LedgerError::InvalidAccount
    );
    let cap = u32_at(data, SPACE + 8);
    require!(
        HEADER.checked_add(cap.checked_mul(SLOT).ok_or(LedgerError::InvalidAccount)?)
            == Some(data.len()),
        LedgerError::InvalidAccount
    );
    require!(len(data) <= cap, LedgerError::InvalidAccount);
    Ok(cap)
}
pub fn len(data: &[u8]) -> usize {
    u32_at(data, SPACE + 12)
}
pub fn initialize(data: &mut [u8]) {
    data[SPACE..SPACE + 8].copy_from_slice(MAP_MAGIC);
    let cap = ((data.len() - HEADER) / SLOT) as u32;
    data[SPACE + 8..SPACE + 12].copy_from_slice(&cap.to_le_bytes());
}
fn search(data: &[u8], key: &Pubkey) -> std::result::Result<usize, usize> {
    let mut left = 0;
    let mut right = len(data);
    while left < right {
        let mid = left + (right - left) / 2;
        let start = HEADER + mid * ENTRY;
        match data[start..start + 32].cmp(key.as_ref()) {
            std::cmp::Ordering::Less => left = mid + 1,
            std::cmp::Ordering::Greater => right = mid,
            std::cmp::Ordering::Equal => return Ok(mid),
        }
    }
    Err(left)
}
pub fn space(capacity: usize) -> usize {
    HEADER + capacity * SLOT
}

pub fn entry(data: &[u8], index: usize) -> Result<(Pubkey, Value)> {
    let parent = Record::deserialize(&mut &data[8..SPACE])
        .map_err(|_| error!(LedgerError::InvalidAccount))?;
    let address = parent.address();
    entry_with_parent(data, index, &parent, &address)
}
pub fn entry_with_parent(
    data: &[u8],
    index: usize,
    parent: &Record,
    parent_address: &Pubkey,
) -> Result<(Pubkey, Value)> {
    require!(index < len(data), LedgerError::InvalidAccount);
    let offset = HEADER + index * ENTRY;
    let key = Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap());
    let value = &data[offset + 32..offset + ENTRY];
    let value = if &value[..8] == REFERENCE {
        Value::Group(Pubkey::new_from_array(value[8..40].try_into().unwrap()))
    } else {
        require!(&value[..8] == MAGIC, LedgerError::InvalidAccount);
        let kind = value[40];
        let registered = value[41] == 1;
        let implicit_allowed = value[42] == 1;
        let sub_index = u32::from_le_bytes(value[43..47].try_into().unwrap());
        let name_len = value[79] as usize;
        let symbol_len = value[144] as usize;
        require!(
            kind <= 3
                && !(registered && kind < 2)
                && value[41] <= 1
                && value[42] <= 1
                && name_len <= 64
                && symbol_len <= 64
                && (sub_index > 0) == registered,
            LedgerError::InvalidAccount
        );
        let string = |bytes: &[u8]| {
            String::from_utf8(bytes.to_vec()).map_err(|_| error!(LedgerError::InvalidAccount))
        };
        Value::Leaf(Record {
            root: parent.root,
            parent: *parent_address,
            relative: Pubkey::new_from_array(value[8..40].try_into().unwrap()),
            custodian: if parent.depth == 2 {
                key
            } else {
                parent.custodian
            },
            kind,
            token_kind: 0,
            depth: parent
                .depth
                .checked_add(1)
                .ok_or(LedgerError::InvalidAccount)?,
            registered,
            implicit_allowed,
            children: 0,
            debit: u128::from_le_bytes(value[47..63].try_into().unwrap()),
            credit: u128::from_le_bytes(value[63..79].try_into().unwrap()),
            name: string(&value[80..80 + name_len])?,
            scope: Pubkey::default(),
            identifier: Pubkey::default(),
            bump: 0,
            sub_index,
            symbol: string(&value[145..145 + symbol_len])?,
            decimals: value[209],
        })
    };
    Ok((key, value))
}
pub fn get(data: &[u8], key: &Pubkey) -> Result<Option<Value>> {
    capacity(data)?;
    search(data, key)
        .ok()
        .map(|i| entry(data, i).map(|(_, value)| value))
        .transpose()
}
pub fn get_with_parent(
    data: &[u8],
    key: &Pubkey,
    parent: &Record,
    parent_address: &Pubkey,
) -> Result<Option<Value>> {
    capacity(data)?;
    search(data, key)
        .ok()
        .map(|i| entry_with_parent(data, i, parent, parent_address).map(|(_, value)| value))
        .transpose()
}
pub fn contains(data: &[u8], key: &Pubkey) -> bool {
    search(data, key).is_ok()
}
pub fn child(data: &[u8], index: u32) -> Result<Option<Pubkey>> {
    let cap = capacity(data)?;
    if index as usize >= cap {
        return Ok(None);
    }
    let offset = HEADER + cap * ENTRY + index as usize * 32;
    let key = Pubkey::new_from_array(data[offset..offset + 32].try_into().unwrap());
    // Child count, rather than a sentinel identity, determines slot validity.
    Ok(Some(key))
}
pub fn set_child(data: &mut [u8], index: u32, relative: Option<Pubkey>) -> Result<()> {
    let cap = capacity(data)?;
    require!((index as usize) < cap, LedgerError::InvalidAccount);
    let offset = HEADER + cap * ENTRY + index as usize * 32;
    data[offset..offset + 32].copy_from_slice(relative.unwrap_or_default().as_ref());
    Ok(())
}
pub fn required_space(data: &[u8], additional: usize, children: usize) -> Result<usize> {
    let cap = capacity(data)?;
    let required = (len(data) + additional).max(children);
    Ok(if required <= cap {
        data.len()
    } else {
        space(required)
    })
}
pub fn grow(data: &mut [u8], old_capacity: usize) {
    let new_capacity = (data.len() - HEADER) / SLOT;
    let old = HEADER + old_capacity * ENTRY;
    let new = HEADER + new_capacity * ENTRY;
    data.copy_within(old..old + old_capacity * 32, new);
    data[old..new].fill(0);
    data[SPACE + 8..SPACE + 12].copy_from_slice(&(new_capacity as u32).to_le_bytes());
}
pub fn put(data: &mut [u8], key: &Pubkey, value: Value) -> Result<()> {
    let cap = capacity(data)?;
    let index = match search(data, key) {
        Ok(index) => index,
        Err(index) => {
            let count = len(data);
            require!(count < cap, LedgerError::InvalidAccount);
            let offset = HEADER + index * ENTRY;
            data.copy_within(offset..HEADER + count * ENTRY, offset + ENTRY);
            data[offset..offset + 32].copy_from_slice(key.as_ref());
            data[SPACE + 12..SPACE + 16].copy_from_slice(&((count + 1) as u32).to_le_bytes());
            index
        }
    };
    let start = HEADER + index * ENTRY + 32;
    let bytes = &mut data[start..start + VALUE];
    bytes.fill(0);
    match value {
        Value::Group(storage) => {
            bytes[..8].copy_from_slice(REFERENCE);
            bytes[8..40].copy_from_slice(storage.as_ref());
        }
        Value::Leaf(record) => {
            require!(
                !record.is_container()
                    && record.children == 0
                    && record.name.len() <= 64
                    && record.symbol.len() <= 64,
                LedgerError::InvalidAccount
            );
            bytes[..8].copy_from_slice(MAGIC);
            bytes[8..40].copy_from_slice(record.relative.as_ref());
            bytes[40] = record.kind;
            bytes[41] = u8::from(record.registered);
            bytes[42] = u8::from(record.implicit_allowed);
            leaf_fields(bytes, &record);
            bytes[79] = record.name.len() as u8;
            bytes[80..80 + record.name.len()].copy_from_slice(record.name.as_bytes());
            bytes[144] = record.symbol.len() as u8;
            bytes[145..145 + record.symbol.len()].copy_from_slice(record.symbol.as_bytes());
            bytes[209] = record.decimals;
        }
    }
    Ok(())
}
fn leaf_fields(bytes: &mut [u8], record: &Record) {
    bytes[43..47].copy_from_slice(&record.sub_index.to_le_bytes());
    bytes[47..63].copy_from_slice(&record.debit.to_le_bytes());
    bytes[63..79].copy_from_slice(&record.credit.to_le_bytes());
}
pub fn update_fields(data: &mut [u8], key: &Pubkey, record: &Record) -> Result<()> {
    capacity(data)?;
    let index = search(data, key).map_err(|_| error!(LedgerError::MissingAccount))?;
    let start = HEADER + index * ENTRY + 32;
    require!(
        &data[start..start + 8] == MAGIC,
        LedgerError::InvalidAccount
    );
    leaf_fields(&mut data[start..start + VALUE], record);
    Ok(())
}
