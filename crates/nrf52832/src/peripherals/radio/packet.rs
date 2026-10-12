// SPDX-License-Identifier: GPL-3.0-only
//! The radio's packet format: the header as RAM holds it and as it goes on air, logical
//! addresses, and the CRC.

// ============================================================================
// Packet configuration
// ============================================================================

/// The fields of PCNF0, PCNF1 and CRCCNF that shape a packet.
#[derive(Clone, Copy)]
pub(crate) struct PacketFormat {
    pub length_field_bits: u32,
    pub s0_bytes: u32,
    pub s1_bits: u32,
    pub s1_include: bool,
    pub maximum_length: u32,
    pub static_length: u32,
    pub base_address_length: u32,
    pub crc_bytes: u32,
    pub crc_skips_address: bool,
}

impl PacketFormat {
    pub fn new(pcnf0: u32, pcnf1: u32, crccnf: u32) -> PacketFormat {
        PacketFormat {
            length_field_bits: pcnf0 & 0xF,
            s0_bytes: (pcnf0 >> 8) & 1,
            s1_bits: (pcnf0 >> 16) & 0xF,
            s1_include: pcnf0 & (1 << 20) != 0,
            maximum_length: pcnf1 & 0xFF,
            static_length: (pcnf1 >> 8) & 0xFF,
            base_address_length: (pcnf1 >> 16) & 7,
            crc_bytes: crccnf & 3,
            crc_skips_address: crccnf & (1 << 8) != 0,
        }
    }

    /// The access address length: BALEN bytes of base (at least one, at most four) and a prefix byte.
    pub fn address_bytes(&self) -> usize {
        self.base_address_length.clamp(1, 4) as usize + 1
    }

    pub fn header_air_bits(&self) -> u64 {
        (self.s0_bytes * 8 + self.length_field_bits + self.s1_bits) as u64
    }

    pub fn crc_bits(&self) -> u64 {
        self.crc_bytes as u64 * 8
    }

    fn length_field_bytes(&self) -> usize {
        self.length_field_bits.div_ceil(8) as usize
    }

    // S1INCL keeps a byte for S1 in RAM even when it has no bits.
    fn s1_ram_bytes(&self) -> usize {
        if self.s1_bits > 0 {
            self.s1_bits.div_ceil(8) as usize
        } else {
            self.s1_include as usize
        }
    }

    /// RAM holds S0, then LENGTH, then S1, each in whole bytes.
    pub fn ram_header_bytes(&self) -> usize {
        self.s0_bytes as usize + self.length_field_bytes() + self.s1_ram_bytes()
    }

    /// On air the header is S0, then LENGTH and S1 packed together from the least significant bit.
    pub fn air_header_bytes(&self) -> usize {
        self.s0_bytes as usize + (self.length_field_bits + self.s1_bits).div_ceil(8) as usize
    }

    /// The payload bytes a header announces, cut to MAXLEN.
    pub fn payload_length(&self, header: &Header) -> usize {
        (header.length + self.static_length as u64).min(self.maximum_length as u64) as usize
    }
}

// ============================================================================
// Header
// ============================================================================

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Header {
    pub s0: u8,
    pub length: u64,
    pub s1: u64,
}

fn bit_mask(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

fn little_endian_value(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .rev()
        .fold(0, |value, &byte| value << 8 | byte as u64)
}

fn store_little_endian(bytes: &mut [u8], value: u64) {
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (value >> (8 * index)) as u8;
    }
}

impl Header {
    pub fn from_ram(format: &PacketFormat, bytes: &[u8]) -> Header {
        let s0_bytes = format.s0_bytes as usize;
        let length_end = s0_bytes + format.length_field_bytes();
        Header {
            s0: if s0_bytes > 0 { bytes[0] } else { 0 },
            length: little_endian_value(&bytes[s0_bytes..length_end]) & bit_mask(format.length_field_bits),
            s1: little_endian_value(&bytes[length_end..format.ram_header_bytes()]) & bit_mask(format.s1_bits),
        }
    }

    pub fn to_ram(self, format: &PacketFormat) -> Vec<u8> {
        let mut bytes = vec![0; format.ram_header_bytes()];
        let s0_bytes = format.s0_bytes as usize;
        if s0_bytes > 0 {
            bytes[0] = self.s0;
        }
        let length_end = s0_bytes + format.length_field_bytes();
        store_little_endian(&mut bytes[s0_bytes..length_end], self.length);
        store_little_endian(&mut bytes[length_end..], self.s1);
        bytes
    }

    pub fn to_air(self, format: &PacketFormat) -> Vec<u8> {
        let mut bytes = vec![0; format.air_header_bytes()];
        let s0_bytes = format.s0_bytes as usize;
        if s0_bytes > 0 {
            bytes[0] = self.s0;
        }
        store_little_endian(
            &mut bytes[s0_bytes..],
            self.length | self.s1 << format.length_field_bits,
        );
        bytes
    }

    pub fn from_air(format: &PacketFormat, bytes: &[u8]) -> Header {
        let s0_bytes = format.s0_bytes as usize;
        let packed = little_endian_value(&bytes[s0_bytes..]);
        Header {
            s0: if s0_bytes > 0 { bytes[0] } else { 0 },
            length: packed & bit_mask(format.length_field_bits),
            s1: (packed >> format.length_field_bits) & bit_mask(format.s1_bits),
        }
    }

    /// TxAdd, bit 6 of S0 in a Bluetooth LE advertising header.
    pub fn transmit_address_random(&self) -> bool {
        self.s0 >> 6 & 1 != 0
    }
}

// ============================================================================
// Addresses and CRC
// ============================================================================

/// A logical address as it goes on air: the top BALEN bytes of its base, least significant first, then its prefix byte.
pub(crate) fn logical_address(format: &PacketFormat, base: u32, prefix: u8) -> Vec<u8> {
    let base_bytes = format.address_bytes() - 1;
    let mut bytes: Vec<u8> = (0..base_bytes)
        .map(|position| (base >> (8 * (4 - base_bytes + position))) as u8)
        .collect();
    bytes.push(prefix);
    bytes
}

/// The radio's CRC: a shift register of LEN bytes fed with each byte from its least significant bit.
pub(crate) fn compute_crc(
    format: &PacketFormat,
    initial: u32,
    polynomial: u32,
    address: &[u8],
    air_header: &[u8],
    payload: &[u8],
) -> u32 {
    let bits = format.crc_bits() as u32;
    if bits == 0 {
        return 0;
    }
    let mask = bit_mask(bits) as u32;
    let polynomial = polynomial & mask;
    let mut crc = initial & mask;
    let address = if format.crc_skips_address {
        &[][..]
    } else {
        address
    };
    for &value in address.iter().chain(air_header).chain(payload) {
        for bit in 0..8 {
            let input = (value >> bit) as u32 & 1;
            let top = (crc >> (bits - 1)) & 1;
            crc = (crc << 1) & mask;
            if input != top {
                crc ^= polynomial;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advertising_format() -> PacketFormat {
        PacketFormat::new(0x0010_0108, 0x0203_00FF, 0x103)
    }

    #[test]
    fn advertising_header_takes_three_bytes_in_ram_and_two_on_air() {
        let format = advertising_format();
        let header = Header::from_ram(&format, &[0x40, 0x09, 0x00]);
        assert_eq!(
            header,
            Header {
                s0: 0x40,
                length: 9,
                s1: 0
            }
        );
        assert_eq!(header.to_air(&format), vec![0x40, 0x09]);
        assert_eq!(Header::from_air(&format, &[0x40, 0x09]), header);
        assert_eq!(header.to_ram(&format), vec![0x40, 0x09, 0x00]);
        assert!(header.transmit_address_random());
    }

    #[test]
    fn logical_address_puts_the_base_before_the_prefix() {
        let format = advertising_format();
        assert_eq!(
            logical_address(&format, 0x89BE_D600, 0x8E),
            vec![0xD6, 0xBE, 0x89, 0x8E]
        );
    }

    #[test]
    fn crc_changes_with_its_seed() {
        let format = advertising_format();
        let first = compute_crc(
            &format,
            0x555555,
            0x65B,
            &[0xD6, 0xBE, 0x89, 0x8E],
            &[0x40, 0x09],
            &[1, 2],
        );
        let second = compute_crc(
            &format,
            0x123456,
            0x65B,
            &[0xD6, 0xBE, 0x89, 0x8E],
            &[0x40, 0x09],
            &[1, 2],
        );
        assert_ne!(first, second);
        assert!(first <= 0xFF_FFFF);
    }
}
