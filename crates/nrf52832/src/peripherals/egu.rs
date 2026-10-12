// SPDX-License-Identifier: GPL-3.0-only
//! The event generator units: each TRIGGER task raises its TRIGGERED event at once, for software
//! interrupts and PPI.

use super::{triggers, EventRegisters};
use crate::context::Context;

const TASKS_TRIGGER: u32 = 0x000;
const TASKS_TRIGGER_END: u32 = 0x040;
const CHANNEL_COUNT: u32 = 16;

pub(crate) struct Egu {
    events: EventRegisters,
}

impl Egu {
    pub fn new(base: u32) -> Egu {
        Egu {
            events: EventRegisters::new(base, (1 << CHANNEL_COUNT) - 1),
        }
    }

    pub fn reset(&mut self) {
        self.events.reset();
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    pub fn read(&self, offset: u32) -> u32 {
        self.events.read(offset).unwrap_or(0)
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        if (TASKS_TRIGGER..TASKS_TRIGGER_END).contains(&offset) && offset.is_multiple_of(4) && triggers(value)
        {
            self.events.raise((offset - TASKS_TRIGGER) / 4, context);
        }
    }
}
