// SPDX-License-Identifier: GPL-3.0-only
//! Decoded instructions kept by address and checked against the halfwords fetched each time, so
//! rewritten or reloaded code is decoded again without any invalidation.

use std::collections::HashMap;

use super::{decode_16, decode_32, is_32bit, Decoded, Instruction, CONDITION_ALWAYS};

// ============================================================================
// Address ranges
// ============================================================================

const FLASH_SIZE: u32 = 0x8_0000;
const RAM_START: u32 = 0x2000_0000;
const RAM_SIZE: u32 = 0x1_0000;

/// The fallback map is emptied once it holds this many instructions.
const OTHER_LIMIT: usize = 16_384;

const EMPTY: Decoded = Decoded {
    instruction: Instruction::Undefined,
    first_halfword: 0,
    second_halfword: 0,
    length: 0,
    condition: CONDITION_ALWAYS,
};

// ============================================================================
// Cache
// ============================================================================

pub struct DecodeCache {
    flash: Vec<Decoded>,
    ram: Vec<Decoded>,
    other: HashMap<u32, Decoded>,
}

impl Default for DecodeCache {
    fn default() -> Self {
        DecodeCache::new()
    }
}

impl DecodeCache {
    pub fn new() -> DecodeCache {
        DecodeCache {
            flash: vec![EMPTY; (FLASH_SIZE / 2) as usize],
            ram: vec![EMPTY; (RAM_SIZE / 2) as usize],
            other: HashMap::new(),
        }
    }

    #[inline(always)]
    fn slot(&mut self, address: u32) -> &mut Decoded {
        if address < FLASH_SIZE {
            return &mut self.flash[(address >> 1) as usize];
        }
        let ram_offset = address.wrapping_sub(RAM_START);
        if ram_offset < RAM_SIZE {
            return &mut self.ram[(ram_offset >> 1) as usize];
        }
        self.other_slot(address)
    }

    #[cold]
    #[inline(never)]
    fn other_slot(&mut self, address: u32) -> &mut Decoded {
        if self.other.len() >= OTHER_LIMIT {
            self.other.clear();
        }
        self.other.entry(address).or_insert(EMPTY)
    }

    /// The instruction at `address`, decoded again if its halfwords changed, or `None` on a bus error.
    #[inline(always)]
    pub fn lookup<E>(
        &mut self,
        address: u32,
        mut fetch: impl FnMut(u32) -> Result<u16, E>,
    ) -> Option<&Decoded> {
        let first = fetch(address).ok()?;
        let slot = self.slot(address);
        if slot.length == 2 && slot.first_halfword == first {
            return Some(slot);
        }
        let second = if is_32bit(first) {
            fetch(address.wrapping_add(2)).ok()?
        } else {
            0
        };
        if slot.length == 4 && slot.first_halfword == first && slot.second_halfword == second {
            return Some(slot);
        }
        *slot = if is_32bit(first) {
            decode_32(address, first, second)
        } else {
            decode_16(address, first)
        };
        Some(slot)
    }

    /// Forgets every decoded instruction.
    pub fn clear(&mut self) {
        self.flash.fill(EMPTY);
        self.ram.fill(EMPTY);
        self.other.clear();
    }
}
