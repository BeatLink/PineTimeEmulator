// SPDX-License-Identifier: GPL-3.0-only
//! The chip's memories: flash, RAM (with its code alias), FICR, UICR and the errata identification block.

use pinetime_core::AccessSize;

// ============================================================================
// Memory map
// ============================================================================

pub const FLASH_SIZE: usize = 0x8_0000;
pub const FLASH_PAGE_SIZE: usize = 0x1000;
pub const RAM_START: u32 = 0x2000_0000;
pub const RAM_SIZE: usize = 0x1_0000;
pub const CODE_RAM_START: u32 = 0x0080_0000;
pub const FICR_START: u32 = 0x1000_0000;
pub const UICR_START: u32 = 0x1000_1000;
pub const ERRATA_START: u32 = 0xF000_0000;

const INFORMATION_BLOCK_SIZE: usize = 0x1000;
const ERASED_BYTE: u8 = 0xFF;

/// Where an address lands in the chip's memories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Region {
    Flash,
    Ram,
    Ficr,
    Uicr,
    Errata,
}

// ============================================================================
// Memory
// ============================================================================

pub(crate) struct Memory {
    pub flash: Vec<u8>,
    pub ram: Vec<u8>,
    pub ficr: Vec<u8>,
    pub uicr: Vec<u8>,
    pub errata: Vec<u8>,
}

impl Memory {
    /// Flash and UICR start erased, and the FICR and errata block hold what `scripts/pinetime.resc` wrote.
    pub fn new() -> Memory {
        let mut memory = Memory {
            flash: vec![ERASED_BYTE; FLASH_SIZE],
            ram: vec![0; RAM_SIZE],
            ficr: vec![ERASED_BYTE; INFORMATION_BLOCK_SIZE],
            uicr: vec![ERASED_BYTE; INFORMATION_BLOCK_SIZE],
            errata: vec![0; INFORMATION_BLOCK_SIZE],
        };
        memory.fill_ficr();
        memory.fill_errata_identification();
        memory
    }

    // Page size and count, the device address, and the part, RAM and flash size.
    fn fill_ficr(&mut self) {
        const FICR_WORDS: [(usize, u32); 14] = [
            (0x010, 0x1000),
            (0x014, 0x80),
            (0x060, 0x5049_4E45),
            (0x064, 0x5449_4D45),
            (0x0A0, 0x1),
            (0x0A4, 0x0A1B_2C3D),
            (0x0A8, 0xC0DE),
            (0x100, 0x52832),
            (0x104, 0x4141_4530),
            (0x108, 0x2000),
            (0x10C, 0x40),
            (0x110, 0x200),
            (0x130, 0x6),
            (0x134, 0x5),
        ];
        for (offset, value) in FICR_WORDS {
            self.ficr[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    // The part and revision numbers older Nordic SDKs read to pick errata workarounds.
    fn fill_errata_identification(&mut self) {
        const ERRATA_WORDS: [(usize, u32); 4] = [(0xFE0, 0x06), (0xFE4, 0x00), (0xFE8, 0x50), (0xFEC, 0x00)];
        for (offset, value) in ERRATA_WORDS {
            self.errata[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    /// Fills RAM with noise, as after the battery is connected; the noise is the same every time.
    pub fn fill_ram_with_noise(&mut self) {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for chunk in self.ram.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let bytes = state.to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }

    // ========================================================================
    // Address lookup
    // ========================================================================

    /// The region and offset of `size` bytes at `address`, if they all fall inside one memory.
    pub fn locate(address: u32, size: u32) -> Option<(Region, usize)> {
        let (region, start, length) = match address >> 28 {
            0x0 if address < FLASH_SIZE as u32 => (Region::Flash, 0, FLASH_SIZE),
            0x0 if (CODE_RAM_START..CODE_RAM_START + RAM_SIZE as u32).contains(&address) => {
                (Region::Ram, CODE_RAM_START, RAM_SIZE)
            }
            0x1 if address < FICR_START + INFORMATION_BLOCK_SIZE as u32 => {
                (Region::Ficr, FICR_START, INFORMATION_BLOCK_SIZE)
            }
            0x1 if (UICR_START..UICR_START + INFORMATION_BLOCK_SIZE as u32).contains(&address) => {
                (Region::Uicr, UICR_START, INFORMATION_BLOCK_SIZE)
            }
            0x2 if address < RAM_START + RAM_SIZE as u32 => (Region::Ram, RAM_START, RAM_SIZE),
            0xF if address < ERRATA_START + INFORMATION_BLOCK_SIZE as u32 => {
                (Region::Errata, ERRATA_START, INFORMATION_BLOCK_SIZE)
            }
            _ => return None,
        };
        let offset = (address - start) as usize;
        if offset + size as usize > length {
            return None;
        }
        Some((region, offset))
    }

    pub fn bytes(&self, region: Region) -> &[u8] {
        match region {
            Region::Flash => &self.flash,
            Region::Ram => &self.ram,
            Region::Ficr => &self.ficr,
            Region::Uicr => &self.uicr,
            Region::Errata => &self.errata,
        }
    }

    pub fn bytes_mut(&mut self, region: Region) -> &mut [u8] {
        match region {
            Region::Flash => &mut self.flash,
            Region::Ram => &mut self.ram,
            Region::Ficr => &mut self.ficr,
            Region::Uicr => &mut self.uicr,
            Region::Errata => &mut self.errata,
        }
    }

    // ========================================================================
    // Plain accesses
    // ========================================================================

    pub fn read_value(bytes: &[u8], offset: usize, size: AccessSize) -> u32 {
        match size {
            AccessSize::Byte => bytes[offset] as u32,
            AccessSize::Halfword => u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as u32,
            AccessSize::Word => u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]),
        }
    }

    pub fn write_value(bytes: &mut [u8], offset: usize, value: u32, size: AccessSize) {
        let length = size.bytes() as usize;
        bytes[offset..offset + length].copy_from_slice(&value.to_le_bytes()[..length]);
    }

    pub fn dma_read_halfword(&self, address: u32) -> u16 {
        let mut bytes = [0; 2];
        self.dma_read(address, &mut bytes);
        u16::from_le_bytes(bytes)
    }

    // ========================================================================
    // EasyDMA
    // ========================================================================

    /// Reads what EasyDMA would see: flash and data RAM; other addresses read as zero.
    pub fn dma_read(&self, address: u32, buffer: &mut [u8]) {
        if let Some((region, offset)) = Self::dma_readable(address, buffer.len()) {
            buffer.copy_from_slice(&self.bytes(region)[offset..offset + buffer.len()]);
            return;
        }
        for (index, byte) in buffer.iter_mut().enumerate() {
            let byte_address = address.wrapping_add(index as u32);
            *byte = match Self::dma_readable(byte_address, 1) {
                Some((region, offset)) => self.bytes(region)[offset],
                None => 0,
            };
        }
    }

    pub fn dma_read_vec(&self, address: u32, length: usize) -> Vec<u8> {
        let mut bytes = vec![0; length];
        self.dma_read(address, &mut bytes);
        bytes
    }

    /// EasyDMA reaches only data RAM, so a write anywhere else (flash included) is dropped; says whether it landed.
    pub fn dma_write(&mut self, address: u32, bytes: &[u8]) -> bool {
        let start = address.wrapping_sub(RAM_START) as usize;
        if address < RAM_START || start + bytes.len() > RAM_SIZE {
            return false;
        }
        self.ram[start..start + bytes.len()].copy_from_slice(bytes);
        true
    }

    fn dma_readable(address: u32, length: usize) -> Option<(Region, usize)> {
        if address < FLASH_SIZE as u32 && address as usize + length <= FLASH_SIZE {
            return Some((Region::Flash, address as usize));
        }
        let start = address.wrapping_sub(RAM_START) as usize;
        if address >= RAM_START && start + length <= RAM_SIZE {
            return Some((Region::Ram, start));
        }
        None
    }
}
