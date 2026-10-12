// SPDX-License-Identifier: GPL-3.0-only
//! Exception state (pending, active, enabled, priority) and the NVIC registers.
//! Ported from InfiniEmu's nvic.c and the exception bookkeeping in cpu.c.

use crate::cpu::CortexM4;

// ============================================================================
// Exception numbers
// ============================================================================

pub const RESET: usize = 1;
pub const NON_MASKABLE_INTERRUPT: usize = 2;
pub const HARD_FAULT: usize = 3;
pub const MEMORY_MANAGEMENT_FAULT: usize = 4;
pub const BUS_FAULT: usize = 5;
pub const USAGE_FAULT: usize = 6;
pub const SUPERVISOR_CALL: usize = 11;
pub const DEBUG_MONITOR: usize = 12;
pub const PENDABLE_SERVICE: usize = 14;
pub const SYSTEM_TICK: usize = 15;
pub const FIRST_EXTERNAL: usize = 16;

pub const EXTERNAL_INTERRUPT_COUNT: usize = 48;
pub const EXCEPTION_COUNT: usize = FIRST_EXTERNAL + EXTERNAL_INTERRUPT_COUNT;
pub const EXTERNAL_LINE_MASK: u64 = (1 << EXTERNAL_INTERRUPT_COUNT) - 1;

/// The exceptions that are always enabled.
const ALWAYS_ENABLED: u64 = (1 << NON_MASKABLE_INTERRUPT)
    | (1 << HARD_FAULT)
    | (1 << SUPERVISOR_CALL)
    | (1 << PENDABLE_SERVICE)
    | (1 << SYSTEM_TICK);

/// The nRF52832 implements the top three bits of each priority.
pub const PRIORITY_BITS: u32 = 3;

// ============================================================================
// Exception state
// ============================================================================

pub(crate) struct ExceptionState {
    /// One bit per exception number.
    pub pending: u64,
    pub active: u64,
    pub enabled: u64,
    /// The configurable priorities, already masked to the implemented bits.
    pub priorities: [u8; EXCEPTION_COUNT],
    pub priority_mask: u8,
    /// The priority below which an exception must be to preempt what runs now.
    pub execution_priority: i32,
    /// Something changed that may let a pending exception preempt.
    pub check_needed: bool,
}

impl ExceptionState {
    pub fn new() -> ExceptionState {
        ExceptionState {
            pending: 0,
            active: 0,
            enabled: ALWAYS_ENABLED,
            priorities: [0; EXCEPTION_COUNT],
            priority_mask: (0xFF << (8 - PRIORITY_BITS)) as u8,
            execution_priority: 256,
            check_needed: true,
        }
    }

    #[inline]
    pub fn priority(&self, number: usize) -> i32 {
        match number {
            RESET => -3,
            NON_MASKABLE_INTERRUPT => -2,
            HARD_FAULT => -1,
            _ => self.priorities[number] as i32,
        }
    }

    pub fn active_count(&self) -> u32 {
        self.active.count_ones()
    }
}

/// The group priority of `priority`: only it decides preemption.
#[inline]
pub(crate) fn group_priority(priority: i32, priority_group: u32) -> i32 {
    if priority < 0 {
        priority
    } else {
        priority & !((2 << priority_group) - 1)
    }
}

impl CortexM4 {
    // ========================================================================
    // Pending and priority bookkeeping
    // ========================================================================

    pub(crate) fn set_exception_pending(&mut self, number: usize) {
        let bit = 1u64 << number;
        if self.exceptions.pending & bit == 0 {
            self.exceptions.pending |= bit;
            self.exceptions.check_needed = true;
            if self.scb.send_event_on_pending() {
                self.event_register = true;
            }
        }
    }

    pub(crate) fn clear_exception_pending(&mut self, number: usize) {
        self.exceptions.pending &= !(1u64 << number);
    }

    /// Marks pending every external interrupt in `mask` (bit 0 is IRQ 0).
    pub(crate) fn pend_external_interrupts(&mut self, mask: u64) {
        let bits = (mask & EXTERNAL_LINE_MASK) << FIRST_EXTERNAL;
        let newly_pending = bits & !self.exceptions.pending;
        if newly_pending != 0 {
            self.exceptions.pending |= newly_pending;
            self.exceptions.check_needed = true;
            if self.scb.send_event_on_pending() {
                self.event_register = true;
            }
        }
    }

    /// Latches risen lines, and every high line whose interrupt is not active, as pending.
    pub(crate) fn update_interrupt_lines(&mut self, lines: u64) {
        let lines = lines & EXTERNAL_LINE_MASK;
        let rising = lines & !self.interrupt_lines;
        self.interrupt_lines = lines;
        let active_lines = self.exceptions.active >> FIRST_EXTERNAL;
        self.pend_external_interrupts((lines & !active_lines) | rising);
    }

    /// Pends again every line still held high whose interrupt is neither pending nor active.
    pub(crate) fn repend_held_lines(&mut self) {
        let active_lines = self.exceptions.active >> FIRST_EXTERNAL;
        self.pend_external_interrupts(self.interrupt_lines & !active_lines);
    }

    /// The level an exception must beat to preempt; without PRIMASK it is the WFI wake level.
    pub(crate) fn compute_execution_priority(&self, with_primask: bool) -> i32 {
        let group = self.scb.priority_group;
        let mut highest = 256;
        let mut active = self.exceptions.active;
        while active != 0 {
            let number = active.trailing_zeros() as usize;
            active &= active - 1;
            highest = highest.min(group_priority(self.exceptions.priority(number), group));
        }
        let mut boosted = 256;
        if self.basepri != 0 {
            boosted = group_priority(self.basepri as i32, group);
        }
        if self.primask && with_primask {
            boosted = 0;
        }
        if self.faultmask {
            boosted = -1;
        }
        highest.min(boosted)
    }

    pub(crate) fn refresh_execution_priority(&mut self) {
        self.exceptions.execution_priority = self.compute_execution_priority(true);
        self.exceptions.check_needed = true;
    }

    /// The pending, enabled exception that would be taken first, with its group priority.
    pub(crate) fn highest_pending_exception(&self) -> Option<(usize, i32)> {
        let group = self.scb.priority_group;
        let mut candidates = self.exceptions.pending & self.exceptions.enabled;
        let mut best: Option<(usize, i32, i32)> = None;
        while candidates != 0 {
            let number = candidates.trailing_zeros() as usize;
            candidates &= candidates - 1;
            let priority = self.exceptions.priority(number);
            let group_level = group_priority(priority, group);
            let better = match best {
                None => true,
                Some((_, best_group, best_priority)) => {
                    group_level < best_group || group_level == best_group && priority < best_priority
                }
            };
            if better {
                best = Some((number, group_level, priority));
            }
        }
        best.map(|(number, group_level, _)| (number, group_level))
    }

    /// The exception that would preempt what runs now, if any.
    pub(crate) fn preempting_exception(&self) -> Option<usize> {
        let (number, group_level) = self.highest_pending_exception()?;
        (group_level < self.exceptions.execution_priority).then_some(number)
    }

    pub(crate) fn set_exception_priority(&mut self, number: usize, value: u8) {
        if number > HARD_FAULT && number < EXCEPTION_COUNT {
            self.exceptions.priorities[number] = value & self.exceptions.priority_mask;
            self.refresh_execution_priority();
        }
    }

    // ========================================================================
    // NVIC registers
    // ========================================================================

    /// Reads the NVIC word at `offset` from 0xE000E100.
    pub(crate) fn read_nvic(&self, offset: u32) -> u32 {
        let external = |field: u64, register: u32| -> u32 {
            let shift = register * 32;
            if shift >= EXTERNAL_INTERRUPT_COUNT as u32 {
                0
            } else {
                ((field >> FIRST_EXTERNAL) >> shift) as u32
            }
        };
        match offset {
            0x000..=0x03F => external(self.exceptions.enabled, offset / 4),
            0x080..=0x0BF => external(self.exceptions.enabled, (offset - 0x080) / 4),
            0x100..=0x13F => external(self.exceptions.pending, (offset - 0x100) / 4),
            0x180..=0x1BF => external(self.exceptions.pending, (offset - 0x180) / 4),
            0x200..=0x23F => external(self.exceptions.active, (offset - 0x200) / 4),
            0x300..=0x3EF => {
                let first = FIRST_EXTERNAL + (offset - 0x300) as usize;
                (0..4).fold(0, |word, lane| {
                    let number = first + lane;
                    let priority = if number < EXCEPTION_COUNT {
                        self.exceptions.priorities[number] as u32
                    } else {
                        0
                    };
                    word | (priority << (lane * 8))
                })
            }
            _ => 0,
        }
    }

    /// Writes the NVIC word at `offset` from 0xE000E100; `lanes` marks the bytes written.
    pub(crate) fn write_nvic(&mut self, offset: u32, value: u32, lanes: u32) {
        let value = value & lanes;
        let external_bits = |register: u32| -> u64 {
            let shift = register * 32;
            if shift >= EXTERNAL_INTERRUPT_COUNT as u32 {
                0
            } else {
                (((value as u64) << shift) & EXTERNAL_LINE_MASK) << FIRST_EXTERNAL
            }
        };
        match offset {
            0x000..=0x03F => {
                self.exceptions.enabled |= external_bits(offset / 4);
                self.exceptions.check_needed = true;
            }
            0x080..=0x0BF => self.exceptions.enabled &= !external_bits((offset - 0x080) / 4),
            0x100..=0x13F => {
                let bits = external_bits((offset - 0x100) / 4);
                self.pend_external_interrupts(bits >> FIRST_EXTERNAL);
            }
            0x180..=0x1BF => {
                self.exceptions.pending &= !external_bits((offset - 0x180) / 4);
                self.repend_held_lines();
            }
            0x300..=0x3EF => {
                let first = FIRST_EXTERNAL + (offset - 0x300) as usize;
                for lane in 0..4 {
                    if lanes & (0xFF << (lane * 8)) != 0 {
                        self.set_exception_priority(first + lane, (value >> (lane * 8)) as u8);
                    }
                }
            }
            _ => {}
        }
    }

    /// The software trigger interrupt register at 0xE000EF00.
    pub(crate) fn write_software_trigger(&mut self, value: u32) {
        let interrupt = (value & 0x1FF) as usize;
        if interrupt < EXTERNAL_INTERRUPT_COUNT {
            self.pend_external_interrupts(1 << interrupt);
        }
    }
}
