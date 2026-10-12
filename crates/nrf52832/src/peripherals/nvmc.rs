// SPDX-License-Identifier: GPL-3.0-only
//! The flash controller: it is always ready, erases pages of flash and UICR to 0xFF, and decides
//! whether the CPU may write flash at all.

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

const WRITE_ENABLED: u32 = 1;
const ERASE_ENABLED: u32 = 2;
const ERASED_BYTE: u8 = 0xFF;

static LAYOUT: [PlainRegister; 4] = [
    plain(0x51C, 0xA, 0x7F),
    plain(0x540, 0, 0x101),
    plain(0x548, 0, u32::MAX),
    plain(0x54C, 0, u32::MAX),
];

// ============================================================================
// NVMC
// ============================================================================

pub(crate) struct Nvmc {
    configuration: u32,
    plain: PlainRegisters,
    ignored_writes: u64,
}

impl Nvmc {
    pub fn new() -> Nvmc {
        Nvmc {
            configuration: 0,
            plain: PlainRegisters::new(&LAYOUT),
            ignored_writes: 0,
        }
    }

    pub fn reset(&mut self) {
        self.configuration = 0;
        self.plain.reset();
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
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        match offset {
            CONFIG => self.configuration = value & 3,
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
