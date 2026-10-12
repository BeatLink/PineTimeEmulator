// SPDX-License-Identifier: GPL-3.0-only
//! The die temperature sensor: each measurement reports the temperature set from outside, in
//! quarter degrees.

use super::{plain, triggers, EventRegisters, PlainRegister, PlainRegisters};
use crate::context::Context;

// ============================================================================
// Registers
// ============================================================================

pub(crate) const BASE: u32 = 0x4000_C000;

const TASKS_START: u32 = 0x000;
const TEMP: u32 = 0x508;
const EVENT_DATARDY: u32 = 0;
const DEFAULT_CELSIUS: f64 = 25.0;

// The calibration constants of the linear temperature model, with the chip's reset values.
static LAYOUT: [PlainRegister; 17] = [
    plain(0x520, 0x320, 0xFFF),
    plain(0x524, 0x343, 0xFFF),
    plain(0x528, 0x35D, 0xFFF),
    plain(0x52C, 0x400, 0xFFF),
    plain(0x530, 0x47F, 0xFFF),
    plain(0x534, 0x37B, 0xFFF),
    plain(0x540, 0x3FCC, 0x3FFF),
    plain(0x544, 0x3F98, 0x3FFF),
    plain(0x548, 0x3F98, 0x3FFF),
    plain(0x54C, 0x0012, 0x3FFF),
    plain(0x550, 0x006A, 0x3FFF),
    plain(0x554, 0x3DD0, 0x3FFF),
    plain(0x560, 0xE2, 0xFF),
    plain(0x564, 0x00, 0xFF),
    plain(0x568, 0x14, 0xFF),
    plain(0x56C, 0x19, 0xFF),
    plain(0x570, 0x50, 0xFF),
];

// ============================================================================
// TEMP
// ============================================================================

pub(crate) struct Temp {
    events: EventRegisters,
    plain: PlainRegisters,
    celsius: f64,
    measured_quarter_degrees: i32,
}

impl Temp {
    pub fn new() -> Temp {
        Temp {
            events: EventRegisters::new(BASE, 1),
            plain: PlainRegisters::new(&LAYOUT),
            celsius: DEFAULT_CELSIUS,
            measured_quarter_degrees: 0,
        }
    }

    /// Resets the registers; the temperature set from outside stays.
    pub fn reset(&mut self) {
        let celsius = self.celsius;
        *self = Temp::new();
        self.celsius = celsius;
    }

    pub fn set_celsius(&mut self, celsius: f64) {
        self.celsius = celsius;
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            TEMP => self.measured_quarter_degrees as u32,
            0x000..0x100 => 0,
            _ => self.plain.read(offset),
        }
    }

    // A measurement completes at once, so STOP has nothing to stop.
    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_START if triggers(value) => {
                self.measured_quarter_degrees = (self.celsius * 4.0).round() as i32;
                self.events.raise(EVENT_DATARDY, context);
            }
            0x000..0x100 | TEMP => {}
            _ => self.plain.write(offset, value),
        }
    }
}
