// SPDX-License-Identifier: GPL-3.0-only
//! The DWT cycle counter and the debug control block registers firmware touches to enable it.
//! Ported from InfiniEmu's dwt.c and dcb.c.

use crate::cpu::CortexM4;

// ============================================================================
// Register bits
// ============================================================================

/// DWT_CTRL at reset: four comparators, cycle counter off.
const CONTROL_RESET: u32 = 0x4000_0000;
const CYCLE_COUNTER_ENABLE: u32 = 1 << 0;
const CONTROL_WRITABLE: u32 = 0x003F_FFFF;

/// DEMCR.TRCENA, which powers the DWT.
const TRACE_ENABLE: u32 = 1 << 24;
const DEMCR_WRITABLE: u32 = 0x010F_07F1;

// ============================================================================
// State
// ============================================================================

pub(crate) struct Debug {
    pub dwt_control: u32,
    pub exception_monitor_control: u32,
    /// CYCCNT when it last started or stopped counting, or was written.
    cycle_counter_base: u32,
    /// The CPU cycle at which `cycle_counter_base` was taken.
    cycle_counter_since: u64,
}

impl Debug {
    pub fn new() -> Debug {
        Debug {
            dwt_control: CONTROL_RESET,
            exception_monitor_control: 0,
            cycle_counter_base: 0,
            cycle_counter_since: 0,
        }
    }

    fn counting(&self) -> bool {
        self.dwt_control & CYCLE_COUNTER_ENABLE != 0 && self.exception_monitor_control & TRACE_ENABLE != 0
    }

    pub fn cycle_counter(&self, now: u64) -> u32 {
        if self.counting() {
            self.cycle_counter_base
                .wrapping_add(now.wrapping_sub(self.cycle_counter_since) as u32)
        } else {
            self.cycle_counter_base
        }
    }

    fn rebase(&mut self, now: u64, value: u32) {
        self.cycle_counter_base = value;
        self.cycle_counter_since = now;
    }
}

impl CortexM4 {
    /// Reads the DWT word at `offset` from 0xE0001000.
    pub(crate) fn read_data_watchpoint(&self, offset: u32) -> u32 {
        match offset {
            0x0 => self.debug.dwt_control,
            0x4 => self.debug.cycle_counter(self.cycles),
            _ => 0,
        }
    }

    /// Writes the DWT word at `offset` from 0xE0001000.
    pub(crate) fn write_data_watchpoint(&mut self, offset: u32, value: u32) {
        let now = self.cycles;
        let current = self.debug.cycle_counter(now);
        match offset {
            0x0 => {
                self.debug.dwt_control = (CONTROL_RESET & !CONTROL_WRITABLE) | (value & CONTROL_WRITABLE);
                self.debug.rebase(now, current);
            }
            0x4 => self.debug.rebase(now, value),
            _ => {}
        }
    }

    /// Reads the debug control block word at `offset` from 0xE000EDF0.
    pub(crate) fn read_debug_control(&self, offset: u32) -> u32 {
        match offset {
            0xC => self.debug.exception_monitor_control,
            _ => 0,
        }
    }

    /// Writes the debug control block word at `offset` from 0xE000EDF0.
    pub(crate) fn write_debug_control(&mut self, offset: u32, value: u32) {
        if offset == 0xC {
            let now = self.cycles;
            let current = self.debug.cycle_counter(now);
            self.debug.exception_monitor_control = value & DEMCR_WRITABLE;
            self.debug.rebase(now, current);
        }
    }
}
