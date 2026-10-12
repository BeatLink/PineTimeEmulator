// SPDX-License-Identifier: GPL-3.0-only
//! The real-time counters: a 24-bit counter on the 32.768 kHz clock with tick, overflow and
//! compare events, worked out from the cycle count instead of ticking.

use pinetime_core::time::LOW_FREQUENCY_HERTZ;
use pinetime_core::{Cycle, CPU_HERTZ};

use super::triggers;
use crate::context::{event_key, peripheral_id, Context};

// ============================================================================
// Registers
// ============================================================================

const TASKS_START: u32 = 0x000;
const TASKS_STOP: u32 = 0x004;
const TASKS_CLEAR: u32 = 0x008;
const TASKS_TRIGOVRFLW: u32 = 0x00C;
const EVENTS_TICK: u32 = 0x100;
const EVENTS_OVRFLW: u32 = 0x104;
const EVENTS_COMPARE: u32 = 0x140;
const INTENSET: u32 = 0x304;
const INTENCLR: u32 = 0x308;
const EVTEN: u32 = 0x340;
const EVTENSET: u32 = 0x344;
const EVTENCLR: u32 = 0x348;
const COUNTER: u32 = 0x504;
const PRESCALER: u32 = 0x508;
const CC: u32 = 0x540;

const COUNTER_MASK: u32 = 0xFF_FFFF;
const TRIGGER_OVERFLOW_VALUE: u32 = 0xFF_FFF0;
const MAXIMUM_COMPARE_CHANNELS: usize = 4;
const TICK_BIT: u32 = 1 << 0;
const OVERFLOW_BIT: u32 = 1 << 1;
const COMPARE_BIT_SHIFT: u32 = 16;
const NO_EVENT: u64 = u64::MAX;

const ALARM_TOKEN: u32 = 0;

const fn compare_bit(channel: usize) -> u32 {
    1 << (COMPARE_BIT_SHIFT as usize + channel)
}

/// The ticks from `counter` until the counter steps onto `target`; a target equal to the counter waits a full wrap.
const fn ticks_until(counter: u32, target: u32) -> u64 {
    let distance = target.wrapping_sub(counter) & COUNTER_MASK;
    if distance == 0 {
        COUNTER_MASK as u64 + 1
    } else {
        distance as u64
    }
}

// ============================================================================
// RTC
// ============================================================================

pub(crate) struct Rtc {
    base: u32,
    compare_channel_count: usize,
    enable_mask: u32,
    running: bool,
    anchor: Cycle,
    base_tick: u64,
    base_count: u32,
    processed_ticks: u64,
    prescaler: u32,
    interrupt_enabled: u32,
    event_routing_enabled: u32,
    events_pending: u32,
    compare_values: [u32; MAXIMUM_COMPARE_CHANNELS],
}

impl Rtc {
    pub fn new(base: u32, compare_channel_count: usize) -> Rtc {
        let compare_bits = ((1 << compare_channel_count) - 1) << COMPARE_BIT_SHIFT;
        Rtc {
            base,
            compare_channel_count,
            enable_mask: TICK_BIT | OVERFLOW_BIT | compare_bits,
            running: false,
            anchor: 0,
            base_tick: 0,
            base_count: 0,
            processed_ticks: 0,
            prescaler: 0,
            interrupt_enabled: 0,
            event_routing_enabled: 0,
            events_pending: 0,
            compare_values: [0; MAXIMUM_COMPARE_CHANNELS],
        }
    }

    pub fn reset(&mut self, context: &mut Context) {
        *self = Rtc::new(self.base, self.compare_channel_count);
        self.anchor = context.now;
        context.cancel(self.alarm_key());
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events_pending & self.interrupt_enabled != 0
    }

    fn alarm_key(&self) -> pinetime_core::EventKey {
        event_key(peripheral_id(self.base), ALARM_TOKEN)
    }

    // ========================================================================
    // Counting
    // ========================================================================

    // One tick lasts (PRESCALER + 1) / 32768 s, which is (PRESCALER + 1) * 64e6 / 32768 cycles.
    fn tick_numerator(&self) -> u128 {
        CPU_HERTZ as u128 * (self.prescaler as u128 + 1)
    }

    fn tick_at(&self, now: Cycle) -> u64 {
        ((now - self.anchor) as u128 * LOW_FREQUENCY_HERTZ as u128 / self.tick_numerator()) as u64
    }

    fn cycle_of_tick(&self, tick: u64) -> Cycle {
        self.anchor + (tick as u128 * self.tick_numerator()).div_ceil(LOW_FREQUENCY_HERTZ as u128) as Cycle
    }

    fn counter_at(&self, tick: u64) -> u32 {
        (self.base_count as u64).wrapping_add(tick - self.base_tick) as u32 & COUNTER_MASK
    }

    fn current_counter(&self, now: Cycle) -> u32 {
        if self.running {
            self.counter_at(self.tick_at(now))
        } else {
            self.base_count
        }
    }

    // Moves the counter to a new value without losing the phase of the 32.768 kHz clock.
    fn set_counter(&mut self, value: u32, now: Cycle) {
        if self.running {
            self.base_tick = self.tick_at(now);
            self.processed_ticks = self.base_tick;
        }
        self.base_count = value & COUNTER_MASK;
    }

    // ========================================================================
    // Events
    // ========================================================================

    // Catches up on every event due by now, so register changes apply only from this moment on.
    fn synchronize(&mut self, context: &mut Context) {
        if self.running {
            let current_tick = self.tick_at(context.now);
            self.process_events_until(current_tick, context);
        }
    }

    fn process_events_until(&mut self, current_tick: u64, context: &mut Context) {
        while self.running {
            let next_tick = self.next_event_tick();
            if next_tick > current_tick {
                break;
            }
            self.processed_ticks = next_tick;
            self.fire_events_at(next_tick, context);
        }
        if current_tick > self.processed_ticks {
            self.processed_ticks = current_tick;
        }
    }

    // A compare fires when the counter steps onto CC, so CC equal to the counter waits a full wrap and CC = COUNTER + 1 fires on the next tick.
    fn next_event_tick(&self) -> u64 {
        let mut next_tick = NO_EVENT;
        let counter = self.counter_at(self.processed_ticks);
        if self.is_worth_scheduling(TICK_BIT) {
            next_tick = self.processed_ticks + 1;
        }
        if self.is_worth_scheduling(OVERFLOW_BIT) {
            next_tick = next_tick.min(self.processed_ticks + ticks_until(counter, 0));
        }
        for channel in 0..self.compare_channel_count {
            if self.is_worth_scheduling(compare_bit(channel)) {
                next_tick =
                    next_tick.min(self.processed_ticks + ticks_until(counter, self.compare_values[channel]));
            }
        }
        next_tick
    }

    // An event is only generated while enabled, and a pending event with no PPI route cannot change anything until it is cleared.
    fn is_worth_scheduling(&self, bit: u32) -> bool {
        if (self.interrupt_enabled | self.event_routing_enabled) & bit == 0 {
            return false;
        }
        self.event_routing_enabled & bit != 0 || self.events_pending & bit == 0
    }

    fn fire_events_at(&mut self, tick: u64, context: &mut Context) {
        let counter = self.counter_at(tick);
        self.fire_event(TICK_BIT, EVENTS_TICK, context);
        if counter == 0 {
            self.fire_event(OVERFLOW_BIT, EVENTS_OVRFLW, context);
        }
        for channel in 0..self.compare_channel_count {
            if counter == self.compare_values[channel] {
                self.fire_event(compare_bit(channel), EVENTS_COMPARE + 4 * channel as u32, context);
            }
        }
    }

    // INTEN or EVTEN lets the event set its register; only EVTEN passes it on to the PPI.
    fn fire_event(&mut self, bit: u32, offset: u32, context: &mut Context) {
        if (self.interrupt_enabled | self.event_routing_enabled) & bit == 0 {
            return;
        }
        self.events_pending |= bit;
        context.touch(peripheral_id(self.base));
        if self.event_routing_enabled & bit != 0 {
            context.raise_event(self.base + offset);
        }
    }

    // One alarm wakes the RTC at its next event, so a quiet RTC costs nothing per tick.
    fn schedule_next_event(&mut self, context: &mut Context) {
        let next_tick = if self.running {
            self.next_event_tick()
        } else {
            NO_EVENT
        };
        if next_tick == NO_EVENT {
            context.cancel(self.alarm_key());
        } else {
            context.schedule(self.alarm_key(), self.cycle_of_tick(next_tick));
        }
    }

    pub fn handle_event(&mut self, _token: u32, context: &mut Context) {
        self.synchronize(context);
        self.schedule_next_event(context);
    }

    // ========================================================================
    // Registers
    // ========================================================================

    fn event_bit(&self, offset: u32) -> Option<u32> {
        match offset {
            EVENTS_TICK => Some(TICK_BIT),
            EVENTS_OVRFLW => Some(OVERFLOW_BIT),
            EVENTS_COMPARE.. if offset < EVENTS_COMPARE + 4 * self.compare_channel_count as u32 => {
                Some(compare_bit(((offset - EVENTS_COMPARE) / 4) as usize))
            }
            _ => None,
        }
    }

    fn compare_channel(&self, offset: u32) -> Option<usize> {
        let channel = (offset.checked_sub(CC)? / 4) as usize;
        (offset.is_multiple_of(4) && channel < self.compare_channel_count).then_some(channel)
    }

    pub fn read(&mut self, offset: u32, context: &mut Context) -> u32 {
        self.synchronize(context);
        if let Some(bit) = self.event_bit(offset) {
            return (self.events_pending & bit != 0) as u32;
        }
        if let Some(channel) = self.compare_channel(offset) {
            return self.compare_values[channel];
        }
        match offset {
            INTENSET | INTENCLR => self.interrupt_enabled,
            EVTEN | EVTENSET | EVTENCLR => self.event_routing_enabled,
            COUNTER => self.current_counter(context.now),
            PRESCALER => self.prescaler,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        self.synchronize(context);
        let now = context.now;
        if let Some(bit) = self.event_bit(offset) {
            if value & 1 != 0 {
                self.events_pending |= bit;
            } else {
                self.events_pending &= !bit;
            }
        } else if let Some(channel) = self.compare_channel(offset) {
            self.compare_values[channel] = value & COUNTER_MASK;
        } else {
            let enable_bits = value & self.enable_mask;
            match offset {
                TASKS_START if triggers(value) && !self.running => {
                    self.running = true;
                    self.anchor = now;
                    self.base_tick = 0;
                    self.processed_ticks = 0;
                }
                TASKS_STOP if triggers(value) && self.running => {
                    self.base_count = self.current_counter(now);
                    self.running = false;
                }
                TASKS_CLEAR if triggers(value) => self.set_counter(0, now),
                TASKS_TRIGOVRFLW if triggers(value) => self.set_counter(TRIGGER_OVERFLOW_VALUE, now),
                INTENSET => self.interrupt_enabled |= enable_bits,
                INTENCLR => self.interrupt_enabled &= !enable_bits,
                EVTEN => self.event_routing_enabled = enable_bits,
                EVTENSET => self.event_routing_enabled |= enable_bits,
                EVTENCLR => self.event_routing_enabled &= !enable_bits,
                // PRESCALER is read-only while the RTC runs.
                PRESCALER if !self.running => self.prescaler = value & 0xFFF,
                _ => {}
            }
        }
        self.schedule_next_event(context);
    }
}
