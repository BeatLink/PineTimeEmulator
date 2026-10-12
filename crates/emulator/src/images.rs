// SPDX-License-Identifier: GPL-3.0-only
//! Reads firmware images (ELF, Intel HEX and raw binaries) into the pieces they place in memory.

use std::fmt;

// ============================================================================
// Pieces
// ============================================================================

/// A run of bytes an image places at one address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub address: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImageError(pub String);

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ImageError {}

fn error(message: impl Into<String>) -> ImageError {
    ImageError(message.into())
}

pub fn binary_pieces(bytes: Vec<u8>, address: u32) -> Vec<Piece> {
    vec![Piece { address, bytes }]
}

// ============================================================================
// ELF
// ============================================================================

const ELF_MAGIC: &[u8] = b"\x7fELF";
const ELF_CLASS_32: u8 = 1;
const ELF_LITTLE_ENDIAN: u8 = 1;
const PROGRAM_HEADER_LOAD: u32 = 1;

/// The loadable segments of a 32-bit little-endian ELF file, placed at their physical addresses.
pub fn elf_pieces(file: &[u8]) -> Result<Vec<Piece>, ImageError> {
    if file.len() < 52 || &file[..4] != ELF_MAGIC {
        return Err(error("not an ELF file"));
    }
    if file[4] != ELF_CLASS_32 || file[5] != ELF_LITTLE_ENDIAN {
        return Err(error("not a 32-bit little-endian ELF file"));
    }

    let header_offset = read_u32(file, 28)? as usize;
    let header_size = read_u16(file, 42)? as usize;
    let header_count = read_u16(file, 44)? as usize;

    let mut pieces = Vec::new();
    for index in 0..header_count {
        let header = header_offset + index * header_size;
        if read_u32(file, header)? != PROGRAM_HEADER_LOAD {
            continue;
        }
        let file_offset = read_u32(file, header + 4)? as usize;
        let physical_address = read_u32(file, header + 12)?;
        let file_size = read_u32(file, header + 16)? as usize;
        if file_size == 0 {
            continue;
        }
        let bytes = file
            .get(file_offset..file_offset + file_size)
            .ok_or_else(|| error("an ELF segment runs past the end of the file"))?;
        pieces.push(Piece {
            address: physical_address,
            bytes: bytes.to_vec(),
        });
    }
    Ok(pieces)
}

fn read_u16(file: &[u8], offset: usize) -> Result<u16, ImageError> {
    let bytes = file
        .get(offset..offset + 2)
        .ok_or_else(|| error("the ELF file is cut short"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(file: &[u8], offset: usize) -> Result<u32, ImageError> {
    let bytes = file
        .get(offset..offset + 4)
        .ok_or_else(|| error("the ELF file is cut short"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

// ============================================================================
// Intel HEX
// ============================================================================

const RECORD_DATA: u8 = 0x00;
const RECORD_END: u8 = 0x01;
const RECORD_EXTENDED_SEGMENT_ADDRESS: u8 = 0x02;
const RECORD_START_SEGMENT_ADDRESS: u8 = 0x03;
const RECORD_EXTENDED_LINEAR_ADDRESS: u8 = 0x04;
const RECORD_START_LINEAR_ADDRESS: u8 = 0x05;

/// The data records of an Intel HEX file, with neighbouring records joined into one piece.
pub fn hex_pieces(text: &str) -> Result<Vec<Piece>, ImageError> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut base_address: u32 = 0;

    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record =
            parse_hex_record(line).map_err(|message| error(format!("line {}: {message}", line_index + 1)))?;
        match record.kind {
            RECORD_DATA => {
                let address = base_address.wrapping_add(record.offset as u32);
                match pieces.last_mut() {
                    Some(last) if last.address.wrapping_add(last.bytes.len() as u32) == address => {
                        last.bytes.extend_from_slice(&record.data)
                    }
                    _ => pieces.push(Piece {
                        address,
                        bytes: record.data,
                    }),
                }
            }
            RECORD_END => break,
            RECORD_EXTENDED_SEGMENT_ADDRESS => base_address = (record_value(&record.data)? as u32) << 4,
            RECORD_EXTENDED_LINEAR_ADDRESS => base_address = (record_value(&record.data)? as u32) << 16,
            RECORD_START_SEGMENT_ADDRESS | RECORD_START_LINEAR_ADDRESS => {}
            other => {
                return Err(error(format!(
                    "line {}: unknown record type {other:#04x}",
                    line_index + 1
                )))
            }
        }
    }
    Ok(pieces)
}

struct HexRecord {
    kind: u8,
    offset: u16,
    data: Vec<u8>,
}

fn parse_hex_record(line: &str) -> Result<HexRecord, String> {
    let digits = line.strip_prefix(':').ok_or("a record must start with ':'")?;
    if digits.len() % 2 != 0 {
        return Err("a record has an odd number of digits".into());
    }
    let bytes = (0..digits.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&digits[index..index + 2], 16).map_err(|_| "a record holds a non-hex digit")
        })
        .collect::<Result<Vec<u8>, _>>()?;
    if bytes.len() < 5 {
        return Err("a record is too short".into());
    }
    let length = bytes[0] as usize;
    if bytes.len() != length + 5 {
        return Err("a record's length does not match its data".into());
    }
    let checksum = bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    if checksum != 0 {
        return Err("a record's checksum is wrong".into());
    }
    Ok(HexRecord {
        kind: bytes[3],
        offset: u16::from_be_bytes([bytes[1], bytes[2]]),
        data: bytes[4..4 + length].to_vec(),
    })
}

fn record_value(data: &[u8]) -> Result<u16, ImageError> {
    match data {
        [high, low] => Ok(u16::from_be_bytes([*high, *low])),
        _ => Err(error("an address record must hold two bytes")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_neighbouring_hex_records_and_follows_extended_addresses() {
        let text = ":0400000001020304F2\n:0400040005060708DE\n:020000041000EA\n:010000009966\n:00000001FF\n";
        let pieces = hex_pieces(text).unwrap();

        assert_eq!(
            pieces,
            [
                Piece {
                    address: 0,
                    bytes: vec![1, 2, 3, 4, 5, 6, 7, 8]
                },
                Piece {
                    address: 0x1000_0000,
                    bytes: vec![0x99]
                },
            ]
        );
    }

    #[test]
    fn rejects_a_hex_record_with_a_bad_checksum() {
        assert!(hex_pieces(":0400000001020304F3\n").is_err());
    }

    #[test]
    fn reads_the_load_segments_of_an_elf_file() {
        let mut file = vec![0u8; 52 + 32 + 4];
        file[..4].copy_from_slice(ELF_MAGIC);
        file[4] = ELF_CLASS_32;
        file[5] = ELF_LITTLE_ENDIAN;
        file[28..32].copy_from_slice(&52u32.to_le_bytes());
        file[42..44].copy_from_slice(&32u16.to_le_bytes());
        file[44..46].copy_from_slice(&1u16.to_le_bytes());
        let header = 52;
        file[header..header + 4].copy_from_slice(&PROGRAM_HEADER_LOAD.to_le_bytes());
        file[header + 4..header + 8].copy_from_slice(&84u32.to_le_bytes());
        file[header + 8..header + 12].copy_from_slice(&0x2000_0000u32.to_le_bytes());
        file[header + 12..header + 16].copy_from_slice(&0x5000u32.to_le_bytes());
        file[header + 16..header + 20].copy_from_slice(&4u32.to_le_bytes());
        file[84..88].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);

        let pieces = elf_pieces(&file).unwrap();

        assert_eq!(
            pieces,
            [Piece {
                address: 0x5000,
                bytes: vec![0xAA, 0xBB, 0xCC, 0xDD]
            }]
        );
    }

    #[test]
    fn refuses_a_file_that_is_not_elf() {
        assert!(elf_pieces(&[0; 60]).is_err());
    }
}
