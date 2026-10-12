// SPDX-License-Identifier: GPL-3.0-only
//! The comparator and the low-power comparator, which share one block: a READY event on START and
//! a result set from outside.

use super::{plain, triggers, EventRegisters, PlainRegister, PlainRegisters};
use crate::context::Context;

pub(crate) const BASE: u32 = 0x4001_3000;

const TASKS_START: u32 = 0x000;
const TASKS_STOP: u32 = 0x004;
const TASKS_SAMPLE: u32 = 0x008;
const SHORTS: u32 = 0x200;
const RESULT: u32 = 0x400;
const ENABLE: u32 = 0x500;

const EVENT_READY: u32 = 0;
const EVENT_DOWN: u32 = 1;
const EVENT_UP: u32 = 2;
const EVENT_CROSS: u32 = 3;

const SHORT_READY_SAMPLE: u32 = 1 << 0;
const SHORT_READY_STOP: u32 = 1 << 1;
const SHORT_DOWN_STOP: u32 = 1 << 2;
const SHORT_UP_STOP: u32 = 1 << 3;
const SHORT_CROSS_STOP: u32 = 1 << 4;

// The last register is an undocumented trim; SystemInit copies the reference ladder calibration from FICR there.
static LAYOUT: [PlainRegister; 11] = [
    plain(0x200, 0, 0x1F),
    plain(0x500, 0, 0x3),
    plain(0x504, 0, 0x7),
    plain(0x508, 0x4, 0xF),
    plain(0x50C, 0, 0x7),
    plain(0x520, 0, 0x3),
    plain(0x530, 0, 0x3F3F),
    plain(0x534, 0, 0x103),
    plain(0x538, 0, 0x1),
    plain(0x53C, 0, 0x3),
    plain(0x540, 0, u32::MAX),
];

pub(crate) struct CompLpcomp {
    events: EventRegisters,
    plain: PlainRegisters,
    started: bool,
    input_above: bool,
    result: bool,
}

impl CompLpcomp {
    pub fn new() -> CompLpcomp {
        CompLpcomp {
            events: EventRegisters::new(BASE, 0xF),
            plain: PlainRegisters::new(&LAYOUT),
            started: false,
            input_above: false,
            result: false,
        }
    }

    /// Resets the registers; the input level set from outside stays.
    pub fn reset(&mut self) {
        let input_above = self.input_above;
        *self = CompLpcomp::new();
        self.input_above = input_above;
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    /// Sets whether the input is above the reference; while started, a change raises UP or DOWN, and CROSS.
    pub fn set_input_above(&mut self, above: bool, context: &mut Context) {
        let changed = above != self.input_above;
        self.input_above = above;
        if !self.started || !changed {
            return;
        }
        self.result = above;
        self.raise(if above { EVENT_UP } else { EVENT_DOWN }, context);
        self.raise(EVENT_CROSS, context);
    }

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            RESULT => self.result as u32,
            0x000..0x100 => 0,
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_START if triggers(value) => {
                self.started = true;
                self.result = self.input_above;
                self.raise(EVENT_READY, context);
            }
            TASKS_STOP if triggers(value) => self.started = false,
            TASKS_SAMPLE if triggers(value) && self.started => self.result = self.input_above,
            0x000..0x100 | RESULT => {}
            _ => {
                self.plain.write(offset, value);
                if offset == ENABLE && value & 3 == 0 {
                    self.started = false;
                }
            }
        }
    }

    fn raise(&mut self, event: u32, context: &mut Context) {
        self.events.raise(event, context);
        let shortcuts = self.plain.read(SHORTS);
        match event {
            EVENT_READY => {
                if shortcuts & SHORT_READY_SAMPLE != 0 {
                    self.result = self.input_above;
                }
                if shortcuts & SHORT_READY_STOP != 0 {
                    self.started = false;
                }
            }
            EVENT_DOWN if shortcuts & SHORT_DOWN_STOP != 0 => self.started = false,
            EVENT_UP if shortcuts & SHORT_UP_STOP != 0 => self.started = false,
            EVENT_CROSS if shortcuts & SHORT_CROSS_STOP != 0 => self.started = false,
            _ => {}
        }
    }
}
