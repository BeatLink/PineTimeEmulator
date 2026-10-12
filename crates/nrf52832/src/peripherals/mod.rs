// SPDX-License-Identifier: GPL-3.0-only
//! The nRF52832 peripherals, one module each, and the register patterns they share.

pub(crate) mod ccm_aar;
pub(crate) mod comp_lpcomp;
pub(crate) mod ecb;
pub(crate) mod egu;
pub(crate) mod gpio;
pub(crate) mod gpiote;
pub(crate) mod mwu;
pub(crate) mod nvmc;
pub(crate) mod power_clock;
pub(crate) mod pwm;
pub(crate) mod qdec;
pub(crate) mod radio;
pub(crate) mod rng;
pub(crate) mod rtc;
pub(crate) mod saadc;
pub(crate) mod serial;
pub(crate) mod temp;
pub(crate) mod timer;
pub(crate) mod uart;
pub(crate) mod watchdog;

use crate::context::Context;

// ============================================================================
// Tasks
// ============================================================================

/// A task runs when a 1 is written to it, which is what the PPI writes.
pub(crate) const fn triggers(value: u32) -> bool {
    value & 1 != 0
}

// ============================================================================
// Events and interrupt enables
// ============================================================================

const EVENTS_START: u32 = 0x100;
const EVENTS_END: u32 = 0x180;
const INTEN: u32 = 0x300;
const INTENSET: u32 = 0x304;
const INTENCLR: u32 = 0x308;

/// The event registers of a peripheral: event n sits at 0x100 + 4n and matches INTEN bit n.
pub(crate) struct EventRegisters {
    base: u32,
    valid: u32,
    pub pending: u32,
    pub enabled: u32,
}

impl EventRegisters {
    pub const fn new(base: u32, valid: u32) -> EventRegisters {
        EventRegisters {
            base,
            valid,
            pending: 0,
            enabled: 0,
        }
    }

    pub fn reset(&mut self) {
        self.pending = 0;
        self.enabled = 0;
    }

    pub const fn address(&self, index: u32) -> u32 {
        self.base + EVENTS_START + 4 * index
    }

    pub const fn is_pending(&self, index: u32) -> bool {
        self.pending & (1 << index) != 0
    }

    pub const fn interrupt_pending(&self) -> bool {
        self.pending & self.enabled != 0
    }

    /// Sets the event and passes it to the PPI.
    pub fn raise(&mut self, index: u32, context: &mut Context) {
        self.pending |= 1 << index;
        context.raise_event(self.address(index));
    }

    fn event_index(&self, offset: u32) -> Option<u32> {
        if !(EVENTS_START..EVENTS_END).contains(&offset) || !offset.is_multiple_of(4) {
            return None;
        }
        let index = (offset - EVENTS_START) / 4;
        (self.valid & (1 << index) != 0).then_some(index)
    }

    /// Reads an event or an interrupt enable register; `None` for any other offset.
    pub fn read(&self, offset: u32) -> Option<u32> {
        match offset {
            INTEN | INTENSET | INTENCLR => Some(self.enabled),
            _ => self
                .event_index(offset)
                .map(|index| self.is_pending(index) as u32),
        }
    }

    /// Writes an event or an interrupt enable register; says whether `offset` was one.
    pub fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            INTEN => self.enabled = value & self.valid,
            INTENSET => self.enabled |= value & self.valid,
            INTENCLR => self.enabled &= !value,
            _ => match self.event_index(offset) {
                Some(index) if value & 1 != 0 => self.pending |= 1 << index,
                Some(index) => self.pending &= !(1 << index),
                None => return false,
            },
        }
        true
    }
}

// ============================================================================
// Plain registers
// ============================================================================

/// A register that only stores what is written, within `mask`, and starts at `reset`.
pub(crate) struct PlainRegister {
    pub offset: u16,
    pub reset: u32,
    pub mask: u32,
}

pub(crate) const fn plain(offset: u16, reset: u32, mask: u32) -> PlainRegister {
    PlainRegister { offset, reset, mask }
}

/// Storage for every register a peripheral gives no behaviour; offsets not in the layout keep all 32 bits.
pub(crate) struct PlainRegisters {
    layout: &'static [PlainRegister],
    values: Vec<(u16, u32)>,
}

impl PlainRegisters {
    pub const fn new(layout: &'static [PlainRegister]) -> PlainRegisters {
        PlainRegisters {
            layout,
            values: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.values.clear();
    }

    fn described(&self, offset: u32) -> Option<&PlainRegister> {
        self.layout
            .iter()
            .find(|register| register.offset as u32 == offset)
    }

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(&(_, value)) = self.values.iter().find(|(stored, _)| *stored as u32 == offset) {
            return value;
        }
        self.described(offset).map_or(0, |register| register.reset)
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        let mask = self.described(offset).map_or(u32::MAX, |register| register.mask);
        let value = value & mask;
        match self
            .values
            .iter_mut()
            .find(|(stored, _)| *stored as u32 == offset)
        {
            Some(entry) => entry.1 = value,
            None => self.values.push((offset as u16, value)),
        }
    }
}
