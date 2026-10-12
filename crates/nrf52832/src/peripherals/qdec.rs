// SPDX-License-Identifier: GPL-3.0-only
//! The quadrature decoder: its registers keep their values and reset values, and no rotation is
//! ever reported.

use super::{plain, triggers, EventRegisters, PlainRegister, PlainRegisters};
use crate::context::Context;

pub(crate) const BASE: u32 = 0x4001_2000;

const TASKS_STOP: u32 = 0x004;
const SAMPLE: u32 = 0x50C;
const ACC: u32 = 0x514;
const ACCREAD: u32 = 0x518;
const ACCDBL: u32 = 0x544;
const ACCDBLREAD: u32 = 0x548;
const EVENT_STOPPED: u32 = 4;

static LAYOUT: [PlainRegister; 10] = [
    plain(0x200, 0, 0x7F),
    plain(0x500, 0, 0x1),
    plain(0x504, 0, 0x1),
    plain(0x508, 0, 0xF),
    plain(0x510, 0, 0xF),
    plain(0x51C, 0xFFFF_FFFF, 0x8000_001F),
    plain(0x520, 0xFFFF_FFFF, 0x8000_001F),
    plain(0x524, 0xFFFF_FFFF, 0x8000_001F),
    plain(0x528, 0, 0x1),
    plain(0x540, 0x10, 0x1FF),
];

pub(crate) struct Qdec {
    events: EventRegisters,
    plain: PlainRegisters,
}

impl Qdec {
    pub fn new() -> Qdec {
        Qdec {
            events: EventRegisters::new(BASE, 0x1F),
            plain: PlainRegisters::new(&LAYOUT),
        }
    }

    pub fn reset(&mut self) {
        *self = Qdec::new();
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    // The accumulators stay at zero, so reading and clearing them copies zero.
    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            SAMPLE | ACC | ACCREAD | ACCDBL | ACCDBLREAD | 0x000..0x100 => 0,
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_STOP if triggers(value) => self.events.raise(EVENT_STOPPED, context),
            SAMPLE | ACC | ACCREAD | ACCDBL | ACCDBLREAD | 0x000..0x100 => {}
            _ => self.plain.write(offset, value),
        }
    }
}
