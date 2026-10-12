// SPDX-License-Identifier: GPL-3.0-only
//! The watchdog: once started it cannot be stopped, and it resets the chip unless every enabled
//! reload register is written in time.

use pinetime_core::time::LOW_FREQUENCY_HERTZ;
use pinetime_core::{Cycle, CPU_HERTZ};

use super::{triggers, EventRegisters};
use crate::chip::ResetKind;
use crate::context::{event_key, Context};

// ============================================================================
// Registers
// ============================================================================

pub(crate) const BASE: u32 = 0x4001_0000;
pub(crate) const ID: u8 = 0x10;

const TASKS_START: u32 = 0x000;
const RUNSTATUS: u32 = 0x400;
const REQSTATUS: u32 = 0x404;
const CRV: u32 = 0x504;
const RREN: u32 = 0x508;
const CONFIG: u32 = 0x50C;
const RR: u32 = 0x600;
const RR_END: u32 = 0x620;

const EVENT_TIMEOUT: u32 = 0;
const RELOAD_MAGIC: u32 = 0x6E52_4635;
const CRV_RESET: u32 = 0xFFFF_FFFF;
const RREN_RESET: u32 = 0x1;
const CONFIG_RESET: u32 = 0x1;

/// With the interrupt enabled the chip gets two 32 kHz ticks to react before the reset.
const TICKS_AFTER_INTERRUPT: u64 = 2;

const TIMEOUT_TOKEN: u32 = 0;

fn cycles_for_ticks(ticks: u64) -> Cycle {
    (ticks as u128 * CPU_HERTZ as u128).div_ceil(LOW_FREQUENCY_HERTZ as u128) as Cycle
}

// ============================================================================
// Watchdog
// ============================================================================

pub(crate) struct Watchdog {
    events: EventRegisters,
    running: bool,
    reload_value: u32,
    reload_requests_enabled: u32,
    configuration: u32,
    reload_requests_pending: u32,
    reset_scheduled: bool,
    deadline: Cycle,
}

impl Watchdog {
    pub fn new() -> Watchdog {
        Watchdog {
            events: EventRegisters::new(BASE, 1),
            running: false,
            reload_value: CRV_RESET,
            reload_requests_enabled: RREN_RESET,
            configuration: CONFIG_RESET,
            reload_requests_pending: 0,
            reset_scheduled: false,
            deadline: 0,
        }
    }

    pub fn reset(&mut self, context: &mut Context) {
        *self = Watchdog::new();
        context.cancel(event_key(ID, TIMEOUT_TOKEN));
    }

    /// Queues the timeout again, for a watchdog that kept running through a reset.
    pub fn reschedule(&mut self, context: &mut Context) {
        if self.running {
            context.schedule(event_key(ID, TIMEOUT_TOKEN), self.deadline);
        }
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    // ========================================================================
    // Counting down
    // ========================================================================

    fn start(&mut self, context: &mut Context) {
        if self.running {
            return;
        }
        self.running = true;
        self.reload_requests_pending = self.reload_requests_enabled;
        self.arm(
            context.now + cycles_for_ticks(self.reload_value as u64 + 1),
            context,
        );
    }

    fn arm(&mut self, deadline: Cycle, context: &mut Context) {
        self.deadline = deadline;
        context.schedule(event_key(ID, TIMEOUT_TOKEN), deadline);
    }

    fn reload(&mut self, register: u32, context: &mut Context) {
        self.reload_requests_pending &= !(1 << register);
        if self.reload_requests_pending != 0 || !self.running || self.reset_scheduled {
            return;
        }
        self.reload_requests_pending = self.reload_requests_enabled;
        self.arm(
            context.now + cycles_for_ticks(self.reload_value as u64 + 1),
            context,
        );
    }

    pub fn handle_event(&mut self, token: u32, context: &mut Context) {
        if token != TIMEOUT_TOKEN || !self.running {
            return;
        }
        if self.reset_scheduled || self.events.enabled & 1 == 0 {
            context.request_reset(ResetKind::Watchdog);
            return;
        }
        self.events.raise(EVENT_TIMEOUT, context);
        self.reset_scheduled = true;
        self.arm(context.now + cycles_for_ticks(TICKS_AFTER_INTERRUPT), context);
    }

    // ========================================================================
    // Registers
    // ========================================================================

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            RUNSTATUS => self.running as u32,
            REQSTATUS if self.running => self.reload_requests_pending,
            REQSTATUS => self.reload_requests_enabled,
            CRV => self.reload_value,
            RREN => self.reload_requests_enabled,
            CONFIG => self.configuration,
            _ => 0,
        }
    }

    // The chip ignores changes to CRV, RREN and CONFIG while the watchdog runs.
    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_START if triggers(value) => self.start(context),
            CRV if !self.running => self.reload_value = value,
            RREN if !self.running => self.reload_requests_enabled = value & 0xFF,
            CONFIG if !self.running => self.configuration = value & 0x9,
            RR..RR_END if value == RELOAD_MAGIC => self.reload((offset - RR) / 4, context),
            _ => {}
        }
    }
}
