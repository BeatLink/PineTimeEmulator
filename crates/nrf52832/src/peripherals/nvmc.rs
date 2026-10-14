// SPDX-License-Identifier: GPL-3.0-only
//! The flash controller: it is always ready, erases pages of flash and UICR to 0xFF, decides
//! whether the CPU may write flash at all, and keeps the instruction cache that saves flash wait states.

use super::{plain, triggers, PlainRegister, PlainRegisters};
use crate::context::Context;
use crate::memory::FLASH_PAGE_SIZE;

// ============================================================================
// Registers
// ============================================================================

const READY: u32 = 0x400;
const READYNEXT: u32 = 0x408;
const CONFIG: u32 = 0x504;
const ERASEPAGE: u32 = 0x508;
const ERASEALL: u32 = 0x50C;
const ERASEPCR0: u32 = 0x510;
const ERASEUICR: u32 = 0x514;
const ICACHECNF: u32 = 0x540;
const IHIT: u32 = 0x548;
const IMISS: u32 = 0x54C;

const WRITE_ENABLED: u32 = 1;
const ERASE_ENABLED: u32 = 2;
const ERASED_BYTE: u8 = 0xFF;

const CACHE_ENABLED: u32 = 1 << 0;
const CACHE_PROFILING: u32 = 1 << 8;

static LAYOUT: [PlainRegister; 1] = [plain(0x51C, 0xA, 0x7F)];

// ============================================================================
// Flash timing
// ============================================================================

/// Wait states a flash read costs at 64 MHz with the cache off (nRF52832 PS v1.4, CPU: WFLASH).
pub const FLASH_WAIT_CYCLES: u32 = 2;
/// Wait states an instruction fetch that misses the cache costs (PS v1.4, CPU: WFLASHCACHE).
pub const CACHE_MISS_WAIT_CYCLES: u32 = 3;
/// The cache holds 2 KB of instructions (PS v1.4, NVMC). Nordic gives neither its layout nor its
/// line size, so it is modelled as direct-mapped with 16-byte lines.
const CACHE_BYTES: usize = 2048;
const CACHE_LINE_BYTES: u32 = 16;
const CACHE_LINES: usize = CACHE_BYTES / CACHE_LINE_BYTES as usize;
const NO_LINE: u32 = u32::MAX;

// ============================================================================
// NVMC
// ============================================================================

pub(crate) struct Nvmc {
    configuration: u32,
    plain: PlainRegisters,
    ignored_writes: u64,
    cache_configuration: u32,
    /// Which flash line each cache slot holds.
    cache_lines: [u32; CACHE_LINES],
    hits: u32,
    misses: u32,
}

impl Nvmc {
    pub fn new() -> Nvmc {
        Nvmc {
            configuration: 0,
            plain: PlainRegisters::new(&LAYOUT),
            ignored_writes: 0,
            cache_configuration: 0,
            cache_lines: [NO_LINE; CACHE_LINES],
            hits: 0,
            misses: 0,
        }
    }

    pub fn reset(&mut self) {
        self.configuration = 0;
        self.plain.reset();
        self.cache_configuration = 0;
        self.cache_lines = [NO_LINE; CACHE_LINES];
        self.hits = 0;
        self.misses = 0;
    }

    /// Whether instruction fetches go through the cache: it must be on, and writing or erasing
    /// flash keeps it invalidated.
    fn cache_in_use(&self) -> bool {
        self.cache_configuration & CACHE_ENABLED != 0 && self.configuration == 0
    }

    /// The wait states fetching the instruction word at a flash `address` costs.
    pub fn fetch_wait_cycles(&mut self, address: u32) -> u32 {
        if !self.cache_in_use() {
            return FLASH_WAIT_CYCLES;
        }
        let line = address / CACHE_LINE_BYTES;
        let slot = &mut self.cache_lines[line as usize % CACHE_LINES];
        let profiling = self.cache_configuration & CACHE_PROFILING != 0;
        if *slot == line {
            // The counters stop at their largest value rather than wrapping, as the chip's do.
            if profiling {
                self.hits = self.hits.saturating_add(1);
            }
            return 0;
        }
        *slot = line;
        if profiling {
            self.misses = self.misses.saturating_add(1);
        }
        CACHE_MISS_WAIT_CYCLES
    }

    /// The wait states a data read or write in flash costs; the cache holds only instructions.
    pub fn data_wait_cycles(&self) -> u32 {
        FLASH_WAIT_CYCLES
    }

    /// Whether CONFIG.WEN lets the CPU write flash and UICR.
    pub fn writes_enabled(&self) -> bool {
        self.configuration == WRITE_ENABLED
    }

    /// Counts a CPU write the controller refused, for the debugger.
    pub fn count_ignored_write(&mut self) {
        self.ignored_writes += 1;
    }

    pub fn ignored_writes(&self) -> u64 {
        self.ignored_writes
    }

    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            READY | READYNEXT => 1,
            CONFIG => self.configuration,
            ERASEPAGE | ERASEALL | ERASEPCR0 | ERASEUICR => 0,
            ICACHECNF => self.cache_configuration,
            IHIT => self.hits,
            IMISS => self.misses,
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        match offset {
            CONFIG => {
                self.configuration = value & 3;
                if self.configuration != 0 {
                    self.cache_lines = [NO_LINE; CACHE_LINES];
                }
            }
            ICACHECNF => {
                self.cache_configuration = value & (CACHE_ENABLED | CACHE_PROFILING);
                // Turning the cache off empties it.
                if self.cache_configuration & CACHE_ENABLED == 0 {
                    self.cache_lines = [NO_LINE; CACHE_LINES];
                }
            }
            IHIT => self.hits = value,
            IMISS => self.misses = value,
            ERASEPAGE | ERASEPCR0 => self.erase_page(value, context),
            ERASEALL if triggers(value) && self.erase_enabled() => {
                context.memory.flash.fill(ERASED_BYTE);
                context.memory.uicr.fill(ERASED_BYTE);
            }
            ERASEUICR if triggers(value) && self.erase_enabled() => context.memory.uicr.fill(ERASED_BYTE),
            ERASEALL | ERASEUICR => {}
            _ => self.plain.write(offset, value),
        }
    }

    fn erase_enabled(&self) -> bool {
        self.configuration == ERASE_ENABLED
    }

    fn erase_page(&mut self, address: u32, context: &mut Context) {
        let page_start = address as usize & !(FLASH_PAGE_SIZE - 1);
        if !self.erase_enabled() || page_start >= context.memory.flash.len() {
            self.ignored_writes += 1;
            return;
        }
        context.memory.flash[page_start..page_start + FLASH_PAGE_SIZE].fill(ERASED_BYTE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: u32 = CACHE_LINE_BYTES;

    fn cached(profiling: bool) -> Nvmc {
        let mut nvmc = Nvmc::new();
        nvmc.cache_configuration = CACHE_ENABLED | if profiling { CACHE_PROFILING } else { 0 };
        nvmc
    }

    #[test]
    fn without_the_cache_every_fetch_and_read_waits() {
        let mut nvmc = Nvmc::new();
        assert_eq!(nvmc.fetch_wait_cycles(0x100), FLASH_WAIT_CYCLES);
        assert_eq!(nvmc.fetch_wait_cycles(0x100), FLASH_WAIT_CYCLES);
        assert_eq!(nvmc.data_wait_cycles(), FLASH_WAIT_CYCLES);
    }

    #[test]
    fn the_cache_misses_once_a_line_then_hits() {
        let mut nvmc = cached(true);
        assert_eq!(nvmc.fetch_wait_cycles(0x100), CACHE_MISS_WAIT_CYCLES);
        assert_eq!(nvmc.fetch_wait_cycles(0x104), 0);
        assert_eq!(nvmc.fetch_wait_cycles(0x100 + LINE), CACHE_MISS_WAIT_CYCLES);
        assert_eq!((nvmc.read(IHIT), nvmc.read(IMISS)), (1, 2));
        // Data reads go round the cache.
        assert_eq!(nvmc.data_wait_cycles(), FLASH_WAIT_CYCLES);
    }

    #[test]
    fn two_lines_a_cache_apart_push_each_other_out() {
        let mut nvmc = cached(false);
        let apart = CACHE_BYTES as u32;
        nvmc.fetch_wait_cycles(0x100);
        nvmc.fetch_wait_cycles(0x100 + apart);
        assert_eq!(nvmc.fetch_wait_cycles(0x100), CACHE_MISS_WAIT_CYCLES);
        // Without profiling the counters stay still.
        assert_eq!((nvmc.read(IHIT), nvmc.read(IMISS)), (0, 0));
    }
}
