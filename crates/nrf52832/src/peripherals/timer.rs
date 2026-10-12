// SPDX-License-Identifier: GPL-3.0-only
//! The timers: a prescaled 16 MHz counter, or a counter of COUNT tasks, with compare, capture and
//! shortcuts, worked out from the cycle count instead of ticking.

use pinetime_core::{Cycle, EventKey};

use super::triggers;
use crate::context::{event_key, peripheral_id, Context};

// ============================================================================
// Registers
// ============================================================================

const TASKS_START: u32 = 0x000;
const TASKS_STOP: u32 = 0x004;
const TASKS_COUNT: u32 = 0x008;
const TASKS_CLEAR: u32 = 0x00C;
const TASKS_SHUTDOWN: u32 = 0x010;
const TASKS_CAPTURE: u32 = 0x040;
const EVENTS_COMPARE: u32 = 0x140;
const SHORTS: u32 = 0x200;
const INTENSET: u32 = 0x304;
const INTENCLR: u32 = 0x308;
const MODE: u32 = 0x504;
const BITMODE: u32 = 0x508;
const PRESCALER: u32 = 0x510;
const CC: u32 = 0x540;

const MAXIMUM_COMPARE_CHANNELS: usize = 6;
const MAXIMUM_PRESCALER: u32 = 9;
const DEFAULT_PRESCALER: u32 = 4;
const STOP_SHORTCUT_SHIFT: u32 = 8;
const INTERRUPT_BIT_SHIFT: u32 = 16;
const NO_EVENT: u64 = u64::MAX;

/// One 16 MHz tick lasts four CPU cycles.
const CYCLES_PER_FAST_TICK: u64 = 4;

const MODE_TIMER: u32 = 0;
const BITMODE_08: u32 = 1;
const BITMODE_24: u32 = 2;
const BITMODE_32: u32 = 3;

const ALARM_TOKEN: u32 = 0;

// ============================================================================
// TIMER
// ============================================================================

pub(crate) struct Timer {
    base: u32,
    compare_channel_count: usize,
    channel_mask: u32,
    running: bool,
    anchor: Cycle,
    base_tick: u64,
    base_count: u32,
    processed_ticks: u64,
    mode: u32,
    bit_mode: u32,
    prescaler: u32,
    shortcuts: u32,
    interrupt_enabled: u32,
    events_pending: u32,
    compare_values: [u32; MAXIMUM_COMPARE_CHANNELS],
}

impl Timer {
    pub fn new(base: u32, compare_channel_count: usize) -> Timer {
        Timer {
            base,
            compare_channel_count,
            channel_mask: (1 << compare_channel_count) - 1,
            running: false,
            anchor: 0,
            base_tick: 0,
            base_count: 0,
            processed_ticks: 0,
            mode: MODE_TIMER,
            bit_mode: 0,
            prescaler: DEFAULT_PRESCALER,
            shortcuts: 0,
            interrupt_enabled: 0,
            events_pending: 0,
            compare_values: [0; MAXIMUM_COMPARE_CHANNELS],
        }
    }

    pub fn reset(&mut self, context: &mut Context) {
        *self = Timer::new(self.base, self.compare_channel_count);
        self.anchor = context.now;
        context.cancel(self.alarm_key());
    }

    pub fn interrupt_pending(&self) -> bool {
        (self.events_pending << INTERRUPT_BIT_SHIFT) & self.interrupt_enabled != 0
    }

    fn alarm_key(&self) -> EventKey {
        event_key(peripheral_id(self.base), ALARM_TOKEN)
    }

    // ========================================================================
    // Counting
    // ========================================================================

    fn counts_time(&self) -> bool {
        self.running && self.mode == MODE_TIMER
    }

    // Prescaler values above 9 behave as 9.
    fn cycles_per_tick(&self) -> u64 {
        CYCLES_PER_FAST_TICK << self.prescaler.min(MAXIMUM_PRESCALER)
    }

    fn tick_at(&self, now: Cycle) -> u64 {
        (now - self.anchor) / self.cycles_per_tick()
    }

    fn counter_mask(&self) -> u32 {
        match self.bit_mode {
            BITMODE_08 => 0xFF,
            BITMODE_24 => 0xFF_FFFF,
            BITMODE_32 => 0xFFFF_FFFF,
            _ => 0xFFFF,
        }
    }

    fn counter_at(&self, tick: u64) -> u32 {
        (self.base_count as u64).wrapping_add(tick - self.base_tick) as u32 & self.counter_mask()
    }

    fn current_counter(&self, now: Cycle) -> u32 {
        if self.counts_time() {
            self.counter_at(self.tick_at(now))
        } else {
            self.base_count
        }
    }

    // Restarts the tick count from now, keeping the counter value, so a new prescaler or mode applies from this moment.
    fn reanchor(&mut self, now: Cycle) {
        self.base_count = self.current_counter(now);
        self.anchor = now;
        self.base_tick = 0;
        self.processed_ticks = 0;
    }

    // ========================================================================
    // Events
    // ========================================================================

    fn synchronize(&mut self, context: &mut Context) {
        if !self.counts_time() {
            return;
        }
        let current_tick = self.tick_at(context.now);
        while self.counts_time() {
            let next_tick = self.next_event_tick();
            if next_tick > current_tick {
                break;
            }
            self.processed_ticks = next_tick;
            let counter = self.counter_at(next_tick);
            if self.apply_compares(counter, context) {
                self.base_tick = next_tick;
                self.base_count = 0;
            }
        }
        if current_tick > self.processed_ticks {
            self.processed_ticks = current_tick;
        }
    }

    // A compare fires when the counter steps onto CC, so writing CC equal to the counter does not fire until the counter wraps.
    fn next_event_tick(&self) -> u64 {
        let counter = self.counter_at(self.processed_ticks);
        let mask = self.counter_mask();
        let modulus = mask as u64 + 1;
        let mut next_tick = NO_EVENT;
        for channel in 0..self.compare_channel_count {
            let target = self.compare_values[channel];
            if target > mask {
                continue;
            }
            let distance = target.wrapping_sub(counter) & mask;
            let ticks = if distance == 0 { modulus } else { distance as u64 };
            next_tick = next_tick.min(self.processed_ticks + ticks);
        }
        next_tick
    }

    /// Raises every compare matching `counter` and runs their STOP shortcuts; says whether a CLEAR shortcut fired.
    fn apply_compares(&mut self, counter: u32, context: &mut Context) -> bool {
        let mut clear = false;
        let mut stop = false;
        for channel in 0..self.compare_channel_count {
            if self.compare_values[channel] != counter {
                continue;
            }
            self.events_pending |= 1 << channel;
            context.raise_event(self.base + EVENTS_COMPARE + 4 * channel as u32);
            clear |= self.shortcuts & (1 << channel) != 0;
            stop |= self.shortcuts & (1 << (channel as u32 + STOP_SHORTCUT_SHIFT)) != 0;
        }
        if stop {
            self.base_count = if clear { 0 } else { counter };
            self.running = false;
        }
        clear
    }

    // One alarm wakes the timer at its next compare, so the counter never ticks one step at a time.
    fn schedule_next_event(&mut self, context: &mut Context) {
        let next_tick = if self.counts_time() {
            self.next_event_tick()
        } else {
            NO_EVENT
        };
        if next_tick == NO_EVENT {
            context.cancel(self.alarm_key());
        } else {
            let cycle = self.anchor + next_tick * self.cycles_per_tick();
            context.schedule(self.alarm_key(), cycle);
        }
    }

    pub fn handle_event(&mut self, _token: u32, context: &mut Context) {
        self.synchronize(context);
        self.schedule_next_event(context);
    }

    // ========================================================================
    // Tasks
    // ========================================================================

    fn start(&mut self, now: Cycle) {
        if !self.running {
            self.running = true;
            self.anchor = now;
            self.base_tick = 0;
            self.processed_ticks = 0;
        }
    }

    fn stop(&mut self, now: Cycle) {
        if self.running {
            self.base_count = self.current_counter(now);
            self.running = false;
        }
    }

    // Clearing keeps the phase of the prescaled clock, so periodic clears never drift.
    fn clear(&mut self, now: Cycle) {
        if self.counts_time() {
            self.base_tick = self.tick_at(now);
            self.processed_ticks = self.base_tick;
        }
        self.base_count = 0;
    }

    // In counter mode each COUNT adds one; in timer mode it does nothing.
    fn count(&mut self, context: &mut Context) {
        if !self.running || self.mode == MODE_TIMER {
            return;
        }
        self.base_count = self.base_count.wrapping_add(1) & self.counter_mask();
        let counter = self.base_count;
        if self.apply_compares(counter, context) {
            self.base_count = 0;
        }
    }

    // ========================================================================
    // Registers
    // ========================================================================

    fn channel_at(&self, offset: u32, start: u32) -> Option<usize> {
        let channel = (offset.checked_sub(start)? / 4) as usize;
        (offset.is_multiple_of(4) && channel < self.compare_channel_count).then_some(channel)
    }

    pub fn read(&mut self, offset: u32, context: &mut Context) -> u32 {
        self.synchronize(context);
        if let Some(channel) = self.channel_at(offset, EVENTS_COMPARE) {
            return (self.events_pending >> channel) & 1;
        }
        if let Some(channel) = self.channel_at(offset, CC) {
            return self.compare_values[channel];
        }
        match offset {
            SHORTS => self.shortcuts,
            INTENSET | INTENCLR => self.interrupt_enabled,
            MODE => self.mode,
            BITMODE => self.bit_mode,
            PRESCALER => self.prescaler,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        self.synchronize(context);
        let now = context.now;
        if let Some(channel) = self.channel_at(offset, EVENTS_COMPARE) {
            if value & 1 != 0 {
                self.events_pending |= 1 << channel;
            } else {
                self.events_pending &= !(1 << channel);
            }
        } else if let Some(channel) = self.channel_at(offset, CC) {
            self.compare_values[channel] = value;
        } else if let Some(channel) = self.channel_at(offset, TASKS_CAPTURE) {
            if triggers(value) {
                self.compare_values[channel] = self.current_counter(now);
            }
        } else {
            match offset {
                TASKS_START if triggers(value) => self.start(now),
                // SHUTDOWN is deprecated on the nRF52832 and stops the timer like STOP.
                TASKS_STOP | TASKS_SHUTDOWN if triggers(value) => self.stop(now),
                TASKS_CLEAR if triggers(value) => self.clear(now),
                TASKS_COUNT if triggers(value) => self.count(context),
                SHORTS => {
                    self.shortcuts = value & (self.channel_mask | self.channel_mask << STOP_SHORTCUT_SHIFT)
                }
                INTENSET => self.interrupt_enabled |= value & (self.channel_mask << INTERRUPT_BIT_SHIFT),
                INTENCLR => self.interrupt_enabled &= !(value & (self.channel_mask << INTERRUPT_BIT_SHIFT)),
                // The chip expects these to change only while stopped; a running timer picks them up from now.
                MODE => {
                    self.reanchor(now);
                    self.mode = value & 3;
                }
                BITMODE => {
                    self.reanchor(now);
                    self.bit_mode = value & 3;
                    self.base_count &= self.counter_mask();
                }
                PRESCALER => {
                    self.reanchor(now);
                    self.prescaler = value & 0xF;
                }
                _ => {}
            }
        }
        self.schedule_next_event(context);
    }
}
