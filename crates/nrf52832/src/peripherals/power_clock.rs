// SPDX-License-Identifier: GPL-3.0-only
//! POWER and CLOCK, which share 0x40000000 and one interrupt: clock start-up, calibration, reset
//! reasons, the retained registers and System OFF.

use pinetime_core::time::cycles_from_milliseconds;

use super::{plain, triggers, EventRegisters, PlainRegister, PlainRegisters};
use crate::context::{event_key, Context};

// ============================================================================
// Registers
// ============================================================================

pub(crate) const BASE: u32 = 0x4000_0000;
pub(crate) const ID: u8 = 0x00;

const TASKS_HFCLKSTART: u32 = 0x000;
const TASKS_HFCLKSTOP: u32 = 0x004;
const TASKS_LFCLKSTART: u32 = 0x008;
const TASKS_LFCLKSTOP: u32 = 0x00C;
const TASKS_CAL: u32 = 0x010;
const TASKS_CTSTART: u32 = 0x014;
const TASKS_CTSTOP: u32 = 0x018;
const RESETREAS: u32 = 0x400;
const HFCLKRUN: u32 = 0x408;
const HFCLKSTAT: u32 = 0x40C;
const LFCLKRUN: u32 = 0x414;
const LFCLKSTAT: u32 = 0x418;
const LFCLKSRCCOPY: u32 = 0x41C;
const RAMSTATUS: u32 = 0x428;
const SYSTEMOFF: u32 = 0x500;
const LFCLKSRC: u32 = 0x518;
const GPREGRET: u32 = 0x51C;
const GPREGRET2: u32 = 0x520;
const CTIV: u32 = 0x538;
const RAM_POWER: u32 = 0x900;
const RAM_POWER_END: u32 = 0x980;

const EVENT_HFCLKSTARTED: u32 = 0;
const EVENT_LFCLKSTARTED: u32 = 1;
const EVENT_DONE: u32 = 3;
const EVENT_CTTO: u32 = 4;
const VALID_EVENTS: u32 = 0x7F;

const RAM_BLOCK_COUNT: usize = 8;
const RAM_POWER_RESET: u32 = 0xFFFF;

const CALIBRATION_TIMER_TOKEN: u32 = 0;
const CALIBRATION_TIMER_STEP_MILLISECONDS: u64 = 250;

pub(crate) const RESET_REASON_PIN: u32 = 1 << 0;
pub(crate) const RESET_REASON_WATCHDOG: u32 = 1 << 1;
pub(crate) const RESET_REASON_SOFT: u32 = 1 << 2;
pub(crate) const RESET_REASON_LOCKUP: u32 = 1 << 3;
pub(crate) const RESET_REASON_SYSTEM_OFF_GPIO: u32 = 1 << 16;

static LAYOUT: [PlainRegister; 5] = [
    plain(0x510, 0, 0x1F),
    plain(0x518, 0, 0x0003_0003),
    plain(0x538, 0, 0x7F),
    plain(0x55C, 0, 0x0003_0003),
    plain(0x578, 0, 0x1),
];

// ============================================================================
// POWER and CLOCK
// ============================================================================

pub(crate) struct PowerClock {
    events: EventRegisters,
    plain: PlainRegisters,
    reset_reasons: u32,
    retained: [u32; 2],
    ram_power: [u32; RAM_BLOCK_COUNT],
    high_frequency_running: bool,
    high_frequency_from_crystal: bool,
    low_frequency_running: bool,
    low_frequency_source_in_use: u32,
    watchdog_running: bool,
    system_off: bool,
}

impl PowerClock {
    pub fn new() -> PowerClock {
        PowerClock {
            events: EventRegisters::new(BASE, VALID_EVENTS),
            plain: PlainRegisters::new(&LAYOUT),
            reset_reasons: 0,
            retained: [0; 2],
            ram_power: [RAM_POWER_RESET; RAM_BLOCK_COUNT],
            high_frequency_running: false,
            high_frequency_from_crystal: false,
            low_frequency_running: false,
            low_frequency_source_in_use: 0,
            watchdog_running: false,
            system_off: false,
        }
    }

    /// Resets everything but GPREGRET and RESETREAS, which survive every reset except a power cycle.
    pub fn reset(&mut self, context: &mut Context) {
        self.events.reset();
        self.plain.reset();
        self.ram_power = [RAM_POWER_RESET; RAM_BLOCK_COUNT];
        self.high_frequency_running = false;
        self.high_frequency_from_crystal = false;
        self.low_frequency_running = self.watchdog_running;
        self.low_frequency_source_in_use = 0;
        self.system_off = false;
        context.cancel(event_key(ID, CALIBRATION_TIMER_TOKEN));
    }

    /// Forgets the reset reasons and the retained registers, as when power is first applied.
    pub fn power_cycle(&mut self) {
        self.reset_reasons = 0;
        self.retained = [0; 2];
    }

    pub fn record_reset_reason(&mut self, reason: u32) {
        self.reset_reasons |= reason;
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    pub fn system_off(&self) -> bool {
        self.system_off
    }

    /// The watchdog keeps the 32 kHz clock on while it runs.
    pub fn set_watchdog_running(&mut self, running: bool) {
        self.watchdog_running = running;
        if running {
            self.low_frequency_running = true;
        }
    }

    // ========================================================================
    // Registers
    // ========================================================================

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            RESETREAS => self.reset_reasons,
            RAMSTATUS => 0xF,
            HFCLKRUN => self.high_frequency_running as u32,
            HFCLKSTAT => (self.high_frequency_from_crystal as u32) * 0x1_0001,
            LFCLKRUN => self.low_frequency_running as u32,
            LFCLKSTAT => self.low_frequency_source_in_use | (self.low_frequency_running as u32) << 16,
            LFCLKSRCCOPY => self.low_frequency_source_in_use,
            GPREGRET => self.retained[0],
            GPREGRET2 => self.retained[1],
            RAM_POWER..RAM_POWER_END if offset.is_multiple_of(16) => {
                self.ram_power[((offset - RAM_POWER) / 16) as usize]
            }
            RAM_POWER..RAM_POWER_END => 0,
            SYSTEMOFF => 0,
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_HFCLKSTART if triggers(value) => {
                self.high_frequency_running = true;
                self.high_frequency_from_crystal = true;
                self.events.raise(EVENT_HFCLKSTARTED, context);
            }
            TASKS_HFCLKSTOP if triggers(value) => self.high_frequency_from_crystal = false,
            TASKS_LFCLKSTART if triggers(value) => {
                self.low_frequency_running = true;
                self.low_frequency_source_in_use = self.plain.read(LFCLKSRC) & 3;
                self.events.raise(EVENT_LFCLKSTARTED, context);
            }
            // The running watchdog keeps the 32 kHz clock on, so LFCLKSTOP does nothing then.
            TASKS_LFCLKSTOP if triggers(value) && !self.watchdog_running => {
                self.low_frequency_running = false
            }
            TASKS_CAL if triggers(value) => self.events.raise(EVENT_DONE, context),
            TASKS_CTSTART if triggers(value) => {
                let steps = self.plain.read(CTIV) as u64;
                let delay = steps * cycles_from_milliseconds(CALIBRATION_TIMER_STEP_MILLISECONDS);
                context.schedule(event_key(ID, CALIBRATION_TIMER_TOKEN), context.now + delay);
            }
            TASKS_CTSTOP if triggers(value) => context.cancel(event_key(ID, CALIBRATION_TIMER_TOKEN)),
            RESETREAS => self.reset_reasons &= !value,
            GPREGRET => self.retained[0] = value & 0xFF,
            GPREGRET2 => self.retained[1] = value & 0xFF,
            SYSTEMOFF if triggers(value) => self.system_off = true,
            RAM_POWER..RAM_POWER_END => {
                let block = ((offset - RAM_POWER) / 16) as usize;
                match offset % 16 {
                    0 => self.ram_power[block] = value,
                    4 => self.ram_power[block] |= value,
                    8 => self.ram_power[block] &= !value,
                    _ => {}
                }
            }
            0x000..0x100 => {}
            _ => self.plain.write(offset, value),
        }
    }

    pub fn handle_event(&mut self, token: u32, context: &mut Context) {
        if token == CALIBRATION_TIMER_TOKEN {
            self.events.raise(EVENT_CTTO, context);
        }
    }
}
