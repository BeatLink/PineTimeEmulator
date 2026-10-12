// SPDX-License-Identifier: GPL-3.0-only
//! SysTick, counting the processor clock: the cycles `run` executes.
//!
//! It keeps the cycle of its next zero instead of counting down, and stops while the core sleeps.

use crate::cpu::CortexM4;
use crate::nvic::SYSTEM_TICK;

// ============================================================================
// Register bits
// ============================================================================

const ENABLE: u32 = 1 << 0;
const TICK_INTERRUPT: u32 = 1 << 1;
const CLOCK_SOURCE: u32 = 1 << 2;
const COUNT_FLAG: u32 = 1 << 16;

/// CALIB: no reference clock, and no exact 10 ms value.
const CALIBRATION: u32 = 0xC000_0000;

const NEVER: u64 = u64::MAX;

// ============================================================================
// State
// ============================================================================

pub(crate) struct SysTick {
    pub enabled: bool,
    pub interrupt: bool,
    pub count_flag: bool,
    pub reload: u32,
    /// The cycle at which the counter next reaches zero, or `NEVER`.
    pub zero_at: u64,
    /// The counter's value while it is stopped.
    pub stopped_value: u32,
}

impl SysTick {
    pub fn new() -> SysTick {
        SysTick {
            enabled: false,
            interrupt: false,
            count_flag: false,
            reload: 0,
            zero_at: NEVER,
            stopped_value: 0,
        }
    }

    fn current_value(&self, now: u64) -> u32 {
        if !self.enabled {
            return self.stopped_value;
        }
        if self.zero_at == NEVER || now >= self.zero_at {
            return 0;
        }
        let remaining = self.zero_at - now;
        (remaining % (self.reload as u64 + 1)) as u32
    }

    /// The cycle the counter reaches zero when it reloads at `now`.
    fn zero_after_reload(&self, now: u64) -> u64 {
        if self.reload == 0 {
            NEVER
        } else {
            now + 1 + self.reload as u64
        }
    }
}

impl CortexM4 {
    /// Handles every time the counter reached zero up to the current cycle.
    pub(crate) fn system_tick_reached_zero(&mut self) {
        let now = self.cycles;
        while self.systick.zero_at <= now {
            self.systick.count_flag = true;
            if self.systick.interrupt {
                self.set_exception_pending(SYSTEM_TICK);
            }
            let reached = self.systick.zero_at;
            self.systick.zero_at = self.systick.zero_after_reload(reached);
        }
    }

    /// Reads the SysTick word at `offset` from 0xE000E010.
    pub(crate) fn read_system_tick(&mut self, offset: u32) -> u32 {
        match offset {
            0x0 => {
                let value = ((self.systick.enabled as u32) * ENABLE)
                    | ((self.systick.interrupt as u32) * TICK_INTERRUPT)
                    | CLOCK_SOURCE
                    | ((self.systick.count_flag as u32) * COUNT_FLAG);
                self.systick.count_flag = false;
                value
            }
            0x4 => self.systick.reload,
            0x8 => self.systick.current_value(self.cycles),
            0xC => CALIBRATION,
            _ => 0,
        }
    }

    /// Writes the SysTick word at `offset` from 0xE000E010.
    pub(crate) fn write_system_tick(&mut self, offset: u32, value: u32) {
        let now = self.cycles;
        match offset {
            0x0 => {
                let enable = value & ENABLE != 0;
                self.systick.interrupt = value & TICK_INTERRUPT != 0;
                if enable && !self.systick.enabled {
                    let current = self.systick.stopped_value;
                    self.systick.enabled = true;
                    self.systick.zero_at = if current == 0 {
                        self.systick.zero_after_reload(now)
                    } else {
                        now + current as u64
                    };
                } else if !enable && self.systick.enabled {
                    self.systick.stopped_value = self.systick.current_value(now);
                    self.systick.enabled = false;
                    self.systick.zero_at = NEVER;
                }
            }
            0x4 => {
                self.systick.reload = value & 0x00FF_FFFF;
                if self.systick.enabled && self.systick.zero_at == NEVER {
                    self.systick.zero_at = self.systick.zero_after_reload(now);
                }
            }
            0x8 => {
                self.systick.count_flag = false;
                self.systick.stopped_value = 0;
                if self.systick.enabled {
                    self.systick.zero_at = self.systick.zero_after_reload(now);
                }
            }
            _ => {}
        }
    }
}
