// SPDX-License-Identifier: GPL-3.0-only
//! The random number generator: while started it produces one byte at a time, each after a short
//! delay, from a fixed-seed generator so runs repeat exactly.

use pinetime_core::time::cycles_from_microseconds;

use super::{triggers, EventRegisters};
use crate::context::{event_key, Context};

// ============================================================================
// Registers
// ============================================================================

pub(crate) const BASE: u32 = 0x4000_D000;
pub(crate) const ID: u8 = 0x0D;

const TASKS_START: u32 = 0x000;
const TASKS_STOP: u32 = 0x004;
const SHORTS: u32 = 0x200;
const CONFIG: u32 = 0x504;
const VALUE: u32 = 0x508;

const EVENT_VALRDY: u32 = 0;
const SHORT_VALRDY_STOP: u32 = 1;
const CONFIG_DERCEN: u32 = 1;

// Typical times for one byte from the product specification, without and with bias correction.
const MICROSECONDS_PER_VALUE: u64 = 30;
const MICROSECONDS_PER_CORRECTED_VALUE: u64 = 120;

const VALUE_TOKEN: u32 = 0;
const SEED: u64 = 0x2545_F491_4F6C_DD1D;

// ============================================================================
// RNG
// ============================================================================

pub(crate) struct Rng {
    events: EventRegisters,
    running: bool,
    shortcuts: u32,
    configuration: u32,
    value: u8,
    state: u64,
}

impl Rng {
    pub fn new() -> Rng {
        Rng {
            events: EventRegisters::new(BASE, 1),
            running: false,
            shortcuts: 0,
            configuration: 0,
            value: 0,
            state: SEED,
        }
    }

    /// Resets the registers; the generator carries on from where it was, so values never repeat after a reset.
    pub fn reset(&mut self, context: &mut Context) {
        let state = self.state;
        *self = Rng::new();
        self.state = state;
        context.cancel(event_key(ID, VALUE_TOKEN));
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    fn schedule_next_value(&self, context: &mut Context) {
        let microseconds = if self.configuration & CONFIG_DERCEN != 0 {
            MICROSECONDS_PER_CORRECTED_VALUE
        } else {
            MICROSECONDS_PER_VALUE
        };
        context.schedule(
            event_key(ID, VALUE_TOKEN),
            context.now + cycles_from_microseconds(microseconds),
        );
    }

    pub fn handle_event(&mut self, _token: u32, context: &mut Context) {
        if !self.running {
            return;
        }
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.value = (self.state.wrapping_mul(SEED) >> 56) as u8;
        self.events.raise(EVENT_VALRDY, context);
        if self.shortcuts & SHORT_VALRDY_STOP != 0 {
            self.running = false;
        } else {
            self.schedule_next_value(context);
        }
    }

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            SHORTS => self.shortcuts,
            CONFIG => self.configuration,
            VALUE => self.value as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_START if triggers(value) && !self.running => {
                self.running = true;
                self.schedule_next_value(context);
            }
            TASKS_STOP if triggers(value) => {
                self.running = false;
                context.cancel(event_key(ID, VALUE_TOKEN));
            }
            SHORTS => self.shortcuts = value & SHORT_VALRDY_STOP,
            CONFIG => self.configuration = value & CONFIG_DERCEN,
            _ => {}
        }
    }
}
