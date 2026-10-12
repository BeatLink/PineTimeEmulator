// SPDX-License-Identifier: GPL-3.0-only
//! The pulse width modulators: play EasyDMA sequences in emulated time and report each channel's
//! duty cycle. Steps shorter than a millisecond are merged, and PWMPERIODEND fires once per merged chunk.

use pinetime_core::EventKey;

use super::{plain, triggers, EventRegisters, PlainRegister, PlainRegisters};
use crate::context::{event_key, peripheral_id, Context};

// ============================================================================
// Registers
// ============================================================================

const TASKS_STOP: u32 = 0x004;
const TASKS_SEQSTART0: u32 = 0x008;
const TASKS_SEQSTART1: u32 = 0x00C;
const TASKS_NEXTSTEP: u32 = 0x010;
const SHORTS: u32 = 0x200;
const ENABLE: u32 = 0x500;
const MODE: u32 = 0x504;
const COUNTERTOP: u32 = 0x508;
const PRESCALER: u32 = 0x50C;
const DECODER: u32 = 0x510;
const LOOP: u32 = 0x514;
const SEQ: u32 = 0x520;
const SEQ_STRIDE: u32 = 0x20;
const PSEL_OUT: u32 = 0x560;

const EVENT_STOPPED: u32 = 1;
const EVENT_SEQSTARTED0: u32 = 2;
const EVENT_SEQEND0: u32 = 4;
const EVENT_SEQEND1: u32 = 5;
const EVENT_PWMPERIODEND: u32 = 6;
const EVENT_LOOPSDONE: u32 = 7;
const VALID_EVENTS: u32 = 0xFE;

const SHORT_SEQEND0_STOP: u32 = 1 << 0;
const SHORT_SEQEND1_STOP: u32 = 1 << 1;
const SHORT_LOOPSDONE_SEQSTART0: u32 = 1 << 2;
const SHORT_LOOPSDONE_SEQSTART1: u32 = 1 << 3;
const SHORT_LOOPSDONE_STOP: u32 = 1 << 4;

const LOAD_COMMON: u32 = 0;
const LOAD_GROUPED: u32 = 1;
const LOAD_INDIVIDUAL: u32 = 2;
const LOAD_WAVEFORM: u32 = 3;
const DECODER_NEXTSTEP: u32 = 1 << 8;

const CHANNEL_COUNT: usize = 4;
const COMPARE_MASK: u16 = 0x7FFF;
const POLARITY_BIT: u16 = 0x8000;
const CYCLES_PER_BASE_TICK: u64 = 4;
const MINIMUM_CHUNK_TICKS: u64 = 16_000;

const STEP_TOKEN: u32 = 0;

static LAYOUT: [PlainRegister; 17] = [
    plain(0x200, 0, 0x1F),
    plain(0x504, 0, 0x1),
    plain(0x508, 0x3FF, 0x7FFF),
    plain(0x50C, 0, 0x7),
    plain(0x510, 0, 0x103),
    plain(0x514, 0, 0xFFFF),
    plain(0x520, 0, u32::MAX),
    plain(0x524, 0, 0x7FFF),
    plain(0x528, 0x1, 0xFF_FFFF),
    plain(0x52C, 0, 0xFF_FFFF),
    plain(0x540, 0, u32::MAX),
    plain(0x544, 0, 0x7FFF),
    plain(0x548, 0x1, 0xFF_FFFF),
    plain(0x54C, 0, 0xFF_FFFF),
    plain(0x560, 0xFFFF_FFFF, 0x8000_001F),
    plain(0x564, 0xFFFF_FFFF, 0x8000_001F),
    plain(0x568, 0xFFFF_FFFF, 0x8000_001F),
];

/// What the step alarm does when it fires.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StepAction {
    None,
    PlaySteps,
    FinishSequence,
}

// With bit 15 set the output starts high and falls at the compare value; clear, it starts low and rises.
fn duty_cycle(value: u16, top: u64) -> f64 {
    if top == 0 {
        return 0.0;
    }
    let compare = ((value & COMPARE_MASK) as u64).min(top);
    let fraction = compare as f64 / top as f64;
    if value & POLARITY_BIT != 0 {
        fraction
    } else {
        1.0 - fraction
    }
}

// ============================================================================
// PWM
// ============================================================================

pub(crate) struct Pwm {
    base: u32,
    events: EventRegisters,
    plain: PlainRegisters,
    enabled: bool,
    running: bool,
    active_sequence: usize,
    step_index: u32,
    step_count: u32,
    loops_remaining: u32,
    waveform_counter_top: u64,
    duty_cycles: [f64; CHANNEL_COUNT],
    pending_step: StepAction,
}

impl Pwm {
    pub fn new(base: u32) -> Pwm {
        Pwm {
            base,
            events: EventRegisters::new(base, VALID_EVENTS),
            plain: PlainRegisters::new(&LAYOUT),
            enabled: false,
            running: false,
            active_sequence: 0,
            step_index: 0,
            step_count: 0,
            loops_remaining: 0,
            waveform_counter_top: 0,
            duty_cycles: [0.0; CHANNEL_COUNT],
            pending_step: StepAction::None,
        }
    }

    pub fn reset(&mut self, context: &mut Context) {
        *self = Pwm::new(self.base);
        context.cancel(self.step_key());
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    /// The fraction of each period, 0 to 1, that the channel's output is high; 0 while stopped.
    pub fn duty_cycle(&self, channel: usize) -> f64 {
        self.duty_cycles.get(channel).copied().unwrap_or(0.0)
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// The GPIO pin a channel drives, or `None` when it is disconnected.
    pub fn output_pin(&self, channel: usize) -> Option<u8> {
        let select = self.plain.read(PSEL_OUT + 4 * channel as u32);
        (channel < CHANNEL_COUNT && select & 0x8000_0000 == 0).then_some((select & 0x1F) as u8)
    }

    fn step_key(&self) -> EventKey {
        event_key(peripheral_id(self.base), STEP_TOKEN)
    }

    fn sequence_register(&self, register: u32) -> u32 {
        self.plain
            .read(SEQ + SEQ_STRIDE * self.active_sequence as u32 + register)
    }

    // ========================================================================
    // Registers
    // ========================================================================

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            ENABLE => self.enabled as u32,
            0x000..0x100 => 0,
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_STOP if triggers(value) => self.stop(context),
            TASKS_SEQSTART0 if triggers(value) => self.start_sequence_task(0, context),
            TASKS_SEQSTART1 if triggers(value) => self.start_sequence_task(1, context),
            TASKS_NEXTSTEP if triggers(value) => self.next_step(context),
            0x000..0x100 => {}
            ENABLE => {
                self.enabled = value & 1 != 0;
                if !self.enabled {
                    self.halt(context);
                }
            }
            _ => self.plain.write(offset, value),
        }
    }

    // ========================================================================
    // Tasks
    // ========================================================================

    // The loop counter is loaded each time playback is started.
    fn start_sequence_task(&mut self, sequence: usize, context: &mut Context) {
        if !self.enabled {
            return;
        }
        self.running = true;
        self.loops_remaining = self.plain.read(LOOP);
        self.begin_sequence(sequence, context);
    }

    fn stop(&mut self, context: &mut Context) {
        self.halt(context);
        self.raise(EVENT_STOPPED, context);
    }

    // Stops pulse generation without raising an event.
    fn halt(&mut self, context: &mut Context) {
        context.cancel(self.step_key());
        self.pending_step = StepAction::None;
        self.running = false;
        self.duty_cycles = [0.0; CHANNEL_COUNT];
    }

    fn next_step(&mut self, context: &mut Context) {
        if !self.running
            || self.plain.read(DECODER) & DECODER_NEXTSTEP == 0
            || self.step_index >= self.step_count
        {
            return;
        }
        self.step_index += 1;
        if self.step_index < self.step_count {
            self.apply_step(self.step_index, context);
            if self.step_index == self.step_count - 1 {
                self.raise(EVENT_SEQEND0 + self.active_sequence as u32, context);
            }
            return;
        }
        // Past the last value, the end delay runs and then the sequence finishes.
        let delay = self.sequence_register(0xC) as u64 * self.period_ticks();
        if delay == 0 {
            self.finish_sequence(context);
        } else {
            self.schedule_step(delay, StepAction::FinishSequence, context);
        }
    }

    // ========================================================================
    // Sequence playback
    // ========================================================================

    fn begin_sequence(&mut self, sequence: usize, context: &mut Context) {
        self.active_sequence = sequence;
        self.step_index = 0;
        self.step_count = self.sequence_register(0x4) / self.values_per_step();
        self.raise(EVENT_SEQSTARTED0 + sequence as u32, context);
        if !self.running {
            return;
        }
        if self.step_count == 0 {
            // A sequence with no complete step lasts one period.
            self.raise(EVENT_SEQEND0 + sequence as u32, context);
            if self.running {
                self.schedule_step(self.period_ticks(), StepAction::FinishSequence, context);
            }
            return;
        }
        if self.plain.read(DECODER) & DECODER_NEXTSTEP != 0 {
            self.apply_step(0, context);
            if self.step_count == 1 {
                self.raise(EVENT_SEQEND0 + sequence as u32, context);
            }
            return;
        }
        self.play_steps(context);
    }

    // Applies the next step and schedules the one after; SEQEND fires when the last value is applied.
    fn play_steps(&mut self, context: &mut Context) {
        if !self.running {
            return;
        }
        self.apply_step(self.step_index, context);
        let mut ticks = self.step_ticks();
        if self.step_index == self.step_count - 1 {
            self.raise(EVENT_SEQEND0 + self.active_sequence as u32, context);
            if self.running {
                let end_delay = self.sequence_register(0xC) as u64 * self.period_ticks();
                self.schedule_step(ticks + end_delay, StepAction::FinishSequence, context);
            }
            return;
        }
        self.step_index += 1;
        while ticks < MINIMUM_CHUNK_TICKS && self.step_index < self.step_count - 1 {
            ticks += self.step_ticks();
            self.step_index += 1;
        }
        self.schedule_step(ticks, StepAction::PlaySteps, context);
    }

    // Runs after a sequence's last value and end delay have played: moves on to the next sequence or loop.
    fn finish_sequence(&mut self, context: &mut Context) {
        // Without looping, the last value keeps playing until STOP.
        if !self.running || self.loops_remaining == 0 {
            return;
        }
        if self.active_sequence == 0 {
            self.begin_sequence(1, context);
            return;
        }
        self.loops_remaining -= 1;
        if self.loops_remaining > 0 {
            self.begin_sequence(0, context);
            return;
        }
        self.raise(EVENT_LOOPSDONE, context);
    }

    fn apply_step(&mut self, step: u32, context: &mut Context) {
        let values_per_step = self.values_per_step();
        let address = self.sequence_register(0).wrapping_add(2 * step * values_per_step);
        let values: Vec<u16> = (0..values_per_step)
            .map(|index| context.memory.dma_read_halfword(address.wrapping_add(2 * index)))
            .collect();
        let top = self.plain.read(COUNTERTOP) as u64;
        self.duty_cycles = match self.plain.read(DECODER) & 3 {
            LOAD_COMMON => [duty_cycle(values[0], top); CHANNEL_COUNT],
            LOAD_GROUPED => {
                let first = duty_cycle(values[0], top);
                let second = duty_cycle(values[1], top);
                [first, first, second, second]
            }
            LOAD_INDIVIDUAL => core::array::from_fn(|channel| duty_cycle(values[channel], top)),
            _ => {
                // The fourth value replaces COUNTERTOP for this step, and channel 3 has no output.
                self.waveform_counter_top = (values[3] & COMPARE_MASK) as u64;
                let top = self.waveform_counter_top;
                [
                    duty_cycle(values[0], top),
                    duty_cycle(values[1], top),
                    duty_cycle(values[2], top),
                    0.0,
                ]
            }
        };
    }

    // ========================================================================
    // Timing
    // ========================================================================

    // One period in 16 MHz ticks: COUNTERTOP prescaled, twice over when counting up and down.
    fn period_ticks(&self) -> u64 {
        let waveform = self.plain.read(DECODER) & 3 == LOAD_WAVEFORM && self.waveform_counter_top != 0;
        let top = if waveform {
            self.waveform_counter_top
        } else {
            self.plain.read(COUNTERTOP) as u64
        };
        let ticks = top.max(1) << self.plain.read(PRESCALER);
        if self.plain.read(MODE) & 1 != 0 {
            2 * ticks
        } else {
            ticks
        }
    }

    // Each value plays for REFRESH + 1 periods.
    fn step_ticks(&self) -> u64 {
        (self.sequence_register(0x8) as u64 + 1) * self.period_ticks()
    }

    fn values_per_step(&self) -> u32 {
        match self.plain.read(DECODER) & 3 {
            LOAD_COMMON => 1,
            LOAD_GROUPED => 2,
            _ => 4,
        }
    }

    fn schedule_step(&mut self, ticks: u64, action: StepAction, context: &mut Context) {
        self.pending_step = action;
        context.schedule(self.step_key(), context.now + ticks.max(1) * CYCLES_PER_BASE_TICK);
    }

    pub fn handle_event(&mut self, _token: u32, context: &mut Context) {
        let action = std::mem::replace(&mut self.pending_step, StepAction::None);
        if !self.running || action == StepAction::None {
            return;
        }
        self.raise(EVENT_PWMPERIODEND, context);
        match action {
            StepAction::PlaySteps => self.play_steps(context),
            StepAction::FinishSequence => self.finish_sequence(context),
            StepAction::None => {}
        }
    }

    // ========================================================================
    // Events and shortcuts
    // ========================================================================

    fn raise(&mut self, event: u32, context: &mut Context) {
        self.events.raise(event, context);
        let shortcuts = self.plain.read(SHORTS);
        match event {
            EVENT_SEQEND0 if shortcuts & SHORT_SEQEND0_STOP != 0 => self.stop(context),
            EVENT_SEQEND1 if shortcuts & SHORT_SEQEND1_STOP != 0 => self.stop(context),
            EVENT_LOOPSDONE => {
                if shortcuts & SHORT_LOOPSDONE_STOP != 0 {
                    self.stop(context);
                } else if shortcuts & SHORT_LOOPSDONE_SEQSTART0 != 0 {
                    self.start_sequence_task(0, context);
                } else if shortcuts & SHORT_LOOPSDONE_SEQSTART1 != 0 {
                    self.start_sequence_task(1, context);
                }
            }
            _ => {}
        }
    }
}
