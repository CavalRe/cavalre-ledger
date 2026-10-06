//! Packed little-endian storage. The namespace selects the layout; there is no
//! discriminator, version, registration bit, relative identity or stored bump.
use crate::ledger_lib::{Header, LedgerConfig, LedgerError, Metadata, Record};
use anchor_lang::prelude::*;

pub const PARENT: usize = 0;
pub const CUSTODIAN: usize = 32;
pub const KIND: usize = 64;
pub const DEPTH: usize = 65;
pub const DEBIT: usize = 66;
pub const CREDIT: usize = 82;
pub const CHILD_INDEX: usize = 98;
pub const HEADER_LEN: usize = 102;
pub const TOKEN_KIND: usize = HEADER_LEN;
pub const IDENTIFIER: usize = TOKEN_KIND + 1;
pub const AUTHORITY: usize = IDENTIFIER + 32;
pub const VAULT: usize = AUTHORITY + 32;
pub const LEDGER_HEADER_LEN: usize = VAULT + 32;
pub const LEAF_LEN: usize = HEADER_LEN + 4;
pub const METADATA_HEADER_LEN: usize = 10;

#[inline(always)]
pub fn key(data: &[u8], at: usize) -> &[u8; 32] {
    data[at..at + 32].try_into().unwrap()
}
#[inline(always)]
pub fn balance(data: &[u8], at: usize) -> u128 {
    u128::from_le_bytes(data[at..at + 16].try_into().unwrap())
}
#[inline(always)]
pub fn set_balance(data: &mut [u8], at: usize, value: u128) {
    data[at..at + 16].copy_from_slice(&value.to_le_bytes());
}
pub fn children_offset(depth: u8) -> usize {
    if depth == 2 {
        LEDGER_HEADER_LEN
    } else {
        HEADER_LEN
    }
}
pub fn child_index(data: &[u8]) -> u32 {
    u32::from_le_bytes(data[CHILD_INDEX..CHILD_INDEX + 4].try_into().unwrap())
}
/// Structural validation only. Callers must separately bind the expected PDA
/// or use a typed pointer established by this program.
pub fn validate(data: &[u8]) -> Result<()> {
    require!(valid_layout(data), LedgerError::InvalidAccount);
    Ok(())
}
/// Allocation-free validation shared by the structural and transfer paths.
#[inline(always)]
pub fn valid_layout(data: &[u8]) -> bool {
    if data.len() < LEAF_LEN {
        return false;
    }
    let depth = data[DEPTH];
    let kind = data[KIND];
    if depth == 0 || kind > 3 || (child_index(data) == 0) != (depth == 1) {
        return false;
    }
    let offset = children_offset(depth);
    if data.len() < offset + 4 {
        return false;
    }
    let count = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
    count
        .checked_mul(32)
        .and_then(|n| n.checked_add(offset + 4))
        == Some(data.len())
        && (kind < 2 || count == 0)
        && (depth != 2 || (kind == 0 && (1..=3).contains(&data[TOKEN_KIND])))
}
pub fn decode_header(data: &[u8]) -> Result<Header> {
    validate(data)?;
    let address = |offset| Pubkey::new_from_array(*key(data, offset));
    let depth = data[DEPTH];
    let offset = children_offset(depth);
    Ok(Header {
        parent: address(PARENT),
        custodian: address(CUSTODIAN),
        kind: data[KIND],
        depth,
        debit: balance(data, DEBIT),
        credit: balance(data, CREDIT),
        child_index: child_index(data),
        ledger: (depth == 2).then(|| LedgerConfig {
            token_kind: data[TOKEN_KIND],
            identifier: address(IDENTIFIER),
            authority: address(AUTHORITY),
            vault: address(VAULT),
        }),
        children: u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()),
    })
}
pub fn decode(data: &[u8]) -> Result<Record> {
    let header = decode_header(data)?;
    Ok(Record {
        parent: header.parent,
        custodian: header.custodian,
        kind: header.kind,
        depth: header.depth,
        debit: header.debit,
        credit: header.credit,
        child_index: header.child_index,
        ledger: header.ledger,
        children: data[children_offset(header.depth) + 4..]
            .as_chunks::<32>()
            .0
            .iter()
            .map(|bytes| Pubkey::new_from_array(*bytes))
            .collect(),
    })
}
/// Write fixed fields and length only; all existing child slots remain in place.
pub fn encode_header(record: &Header, data: &mut [u8]) -> Result<()> {
    require!(data.len() == record.space(), LedgerError::InvalidAccount);
    data[PARENT..PARENT + 32].copy_from_slice(record.parent.as_ref());
    data[CUSTODIAN..CUSTODIAN + 32].copy_from_slice(record.custodian.as_ref());
    data[KIND] = record.kind;
    data[DEPTH] = record.depth;
    set_balance(data, DEBIT, record.debit);
    set_balance(data, CREDIT, record.credit);
    data[CHILD_INDEX..CHILD_INDEX + 4].copy_from_slice(&record.child_index.to_le_bytes());
    if let Some(config) = &record.ledger {
        require!(record.depth == 2, LedgerError::InvalidAccount);
        data[TOKEN_KIND] = config.token_kind;
        for (offset, value) in [
            (IDENTIFIER, config.identifier),
            (AUTHORITY, config.authority),
            (VAULT, config.vault),
        ] {
            data[offset..offset + 32].copy_from_slice(value.as_ref());
        }
    } else {
        require!(record.depth != 2, LedgerError::InvalidAccount);
    }
    let offset = children_offset(record.depth);
    data[offset..offset + 4].copy_from_slice(&record.children.to_le_bytes());
    validate(data)
}
pub fn encode(record: &Record, data: &mut [u8]) -> Result<()> {
    encode_header(&record.header(), data)?;
    let offset = children_offset(record.depth);
    for (bytes, child) in data[offset + 4..]
        .as_chunks_mut::<32>()
        .0
        .iter_mut()
        .zip(&record.children)
    {
        bytes.copy_from_slice(child.as_ref());
    }
    validate(data)
}
/// Read one slot from a validated accounting record, without decoding siblings.
pub fn child_at(data: &[u8], index: u32) -> Result<Pubkey> {
    validate(data)?;
    let offset = children_offset(data[DEPTH]);
    let count = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    require!(index < count, LedgerError::InvalidAccount);
    Ok(Pubkey::new_from_array(*key(
        data,
        offset + 4 + 32 * index as usize,
    )))
}
/// Write one slot after the account has been resized and its length updated.
pub fn set_child(data: &mut [u8], index: u32, relative: &Pubkey) -> Result<()> {
    let offset = children_offset(data[DEPTH]);
    let count = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    require!(index < count, LedgerError::InvalidAccount);
    let at = offset + 4 + 32 * index as usize;
    data.get_mut(at..at + 32)
        .ok_or_else(|| error!(LedgerError::InvalidAccount))?
        .copy_from_slice(relative.as_ref());
    Ok(())
}
pub fn encode_metadata(metadata: &Metadata) -> Result<Vec<u8>> {
    require!(
        metadata.name.len() <= 64 && metadata.symbol.len() <= 64,
        LedgerError::InvalidName
    );
    let mut data =
        Vec::with_capacity(METADATA_HEADER_LEN + metadata.name.len() + metadata.symbol.len());
    data.push(metadata.bump);
    data.push(metadata.decimals);
    for value in [&metadata.name, &metadata.symbol] {
        data.extend_from_slice(&(value.len() as u32).to_le_bytes());
        data.extend_from_slice(value.as_bytes());
    }
    Ok(data)
}
pub fn decode_metadata(data: &[u8]) -> Result<Metadata> {
    require!(
        data.len() >= METADATA_HEADER_LEN,
        LedgerError::InvalidAccount
    );
    let mut fields = &data[2..];
    fn string(fields: &mut &[u8]) -> Result<String> {
        require!(fields.len() >= 4, LedgerError::InvalidAccount);
        let len = u32::from_le_bytes(fields[..4].try_into().unwrap()) as usize;
        require!(
            len <= 64 && fields.len() >= 4 + len,
            LedgerError::InvalidAccount
        );
        let value = std::str::from_utf8(&fields[4..4 + len])
            .map_err(|_| error!(LedgerError::InvalidAccount))?
            .to_owned();
        *fields = &fields[4 + len..];
        Ok(value)
    }
    let name = string(&mut fields)?;
    let symbol = string(&mut fields)?;
    require!(fields.is_empty(), LedgerError::InvalidAccount);
    Ok(Metadata {
        bump: data[0],
        decimals: data[1],
        name,
        symbol,
    })
}
