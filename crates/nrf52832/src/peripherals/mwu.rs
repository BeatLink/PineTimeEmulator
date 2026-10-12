// SPDX-License-Identifier: GPL-3.0-only
//! The memory watch unit: its region, enable and event registers read back, but no memory access
//! is watched.

use super::{EventRegisters, PlainRegisters};
use crate::context::Context;

pub(crate) const BASE: u32 = 0x4002_0000;

const NMIEN: u32 = 0x320;
const REGIONEN: u32 = 0x510;
const PERREGION_SUBSTAT: u32 = 0x400;
const PERREGION_SUBSTAT_END: u32 = 0x410;
const PREGION: u32 = 0x6C0;
const PREGION_END: u32 = 0x6E0;

/// Event n lives at 0x100 + 4n and matches bit n of INTEN, NMIEN and REGIONEN.
const VALID_EVENTS: u32 = 0x0F00_00FF;

pub(crate) struct Mwu {
    events: EventRegisters,
    plain: PlainRegisters,
    non_maskable_interrupt_enabled: u32,
    regions_enabled: u32,
    subregion_status: [u32; 4],
}

/// Applies a write to a plain enable register or its SET and CLR companions at the next two words.
fn write_enable_trio(register: &mut u32, offset_in_trio: u32, value: u32) {
    match offset_in_trio {
        0 => *register = value & VALID_EVENTS,
        4 => *register |= value & VALID_EVENTS,
        _ => *register &= !value,
    }
}

impl Mwu {
    pub fn new() -> Mwu {
        Mwu {
            events: EventRegisters::new(BASE, VALID_EVENTS),
            plain: PlainRegisters::new(&[]),
            non_maskable_interrupt_enabled: 0,
            regions_enabled: 0,
            subregion_status: [0; 4],
        }
    }

    pub fn reset(&mut self) {
        *self = Mwu::new();
    }

    /// The MWU interrupt; the SoftDevice may route regions to the NMI instead, which this model never raises.
    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    /// Raises a watch event by hand: bits 0 to 7 are REGION[0..3].WA/RA, bits 24 to 27 are PREGION[0..1].WA/RA.
    pub fn trigger_event(&mut self, bit: u32, context: &mut Context) {
        if bit < 32 && VALID_EVENTS & (1 << bit) != 0 {
            self.events.raise(bit, context);
        }
    }

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            NMIEN..0x32C => self.non_maskable_interrupt_enabled,
            REGIONEN..0x51C => self.regions_enabled,
            PERREGION_SUBSTAT..PERREGION_SUBSTAT_END => {
                self.subregion_status[((offset - PERREGION_SUBSTAT) / 4) as usize]
            }
            // PREGION START and END are reserved and read as zero; SUBS picks the watched subregions.
            PREGION..PREGION_END if offset % 16 < 8 => 0,
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            NMIEN..0x32C => {
                write_enable_trio(&mut self.non_maskable_interrupt_enabled, offset - NMIEN, value)
            }
            REGIONEN..0x51C => write_enable_trio(&mut self.regions_enabled, offset - REGIONEN, value),
            // The per-subregion access status bits are cleared by writing 1.
            PERREGION_SUBSTAT..PERREGION_SUBSTAT_END => {
                self.subregion_status[((offset - PERREGION_SUBSTAT) / 4) as usize] &= !value;
            }
            PREGION..PREGION_END if offset % 16 < 8 => {}
            _ => self.plain.write(offset, value),
        }
    }
}
