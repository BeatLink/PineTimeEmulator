// SPDX-License-Identifier: GPL-3.0-only
//! The successive approximation ADC: converts the voltages the board supplies and writes 16-bit
//! results to RAM through EasyDMA.

use super::{plain, triggers, EventRegisters, PlainRegister, PlainRegisters};
use crate::context::{event_key, Context};

// ============================================================================
// Registers
// ============================================================================

pub(crate) const BASE: u32 = 0x4000_7000;
pub(crate) const ID: u8 = 0x07;

const TASKS_START: u32 = 0x000;
const TASKS_SAMPLE: u32 = 0x004;
const TASKS_STOP: u32 = 0x008;
const TASKS_CALIBRATEOFFSET: u32 = 0x00C;
const STATUS: u32 = 0x400;
const ENABLE: u32 = 0x500;
const CH: u32 = 0x510;
const CH_STRIDE: u32 = 0x10;
const RESOLUTION: u32 = 0x5F0;
const OVERSAMPLE: u32 = 0x5F4;
const SAMPLERATE: u32 = 0x5F8;
const RESULT_PTR: u32 = 0x62C;
const RESULT_MAXCNT: u32 = 0x630;
const RESULT_AMOUNT: u32 = 0x634;

const EVENT_STARTED: u32 = 0;
const EVENT_END: u32 = 1;
const EVENT_DONE: u32 = 2;
const EVENT_RESULTDONE: u32 = 3;
const EVENT_CALIBRATEDONE: u32 = 4;
const EVENT_STOPPED: u32 = 5;
const EVENT_CH0_LIMITH: u32 = 6;
const EVENT_CH0_LIMITL: u32 = 7;
const VALID_EVENTS: u32 = (1 << 22) - 1;

const CHANNEL_COUNT: usize = 8;
const PSEL_NOT_CONNECTED: u32 = 0;
const PSEL_VDD: u32 = 9;
const MAXIMUM_OVERSAMPLE: u32 = 8;
const SAMPLE_RATE_COMPARE_MINIMUM: u64 = 80;
const INTERNAL_REFERENCE_VOLTS: f64 = 0.6;
const DEFAULT_SUPPLY_VOLTS: f64 = 3.3;
const CYCLES_PER_SAMPLE_CLOCK_TICK: u64 = 4;
const GAIN_FACTORS: [f64; 8] = [
    1.0 / 6.0,
    1.0 / 5.0,
    1.0 / 4.0,
    1.0 / 3.0,
    1.0 / 2.0,
    1.0,
    2.0,
    4.0,
];

const SAMPLE_TIMER_TOKEN: u32 = 0;

static LAYOUT: [PlainRegister; 36] = {
    let mut layout = [const { plain(0, 0, 0) }; 36];
    let mut channel = 0;
    while channel < CHANNEL_COUNT {
        let offset = (CH + CH_STRIDE * channel as u32) as u16;
        layout[4 * channel] = plain(offset, 0, 0x1F);
        layout[4 * channel + 1] = plain(offset + 4, 0, 0x1F);
        layout[4 * channel + 2] = plain(offset + 8, 0x0002_0000, 0x0117_1733);
        layout[4 * channel + 3] = plain(offset + 12, 0x7FFF_8000, u32::MAX);
        channel += 1;
    }
    layout[32] = plain(RESOLUTION as u16, 0x1, 0x7);
    layout[33] = plain(OVERSAMPLE as u16, 0, 0xF);
    layout[34] = plain(SAMPLERATE as u16, 0, 0x17FF);
    layout[35] = plain(RESULT_MAXCNT as u16, 0, 0x7FFF);
    layout
};

// ============================================================================
// SAADC
// ============================================================================

pub(crate) struct Saadc {
    events: EventRegisters,
    plain: PlainRegisters,
    enabled: bool,
    supply_volts: f64,
    buffer_active: bool,
    buffer_pointer: u32,
    buffer_maximum_count: u32,
    result_amount: u32,
    oversample_progress: u32,
    sample_timer_running: bool,
    sample_period: u64,
}

impl Saadc {
    pub fn new() -> Saadc {
        Saadc {
            events: EventRegisters::new(BASE, VALID_EVENTS),
            plain: PlainRegisters::new(&LAYOUT),
            enabled: false,
            supply_volts: DEFAULT_SUPPLY_VOLTS,
            buffer_active: false,
            buffer_pointer: 0,
            buffer_maximum_count: 0,
            result_amount: 0,
            oversample_progress: 0,
            sample_timer_running: false,
            sample_period: 0,
        }
    }

    /// Resets the registers; the supply voltage set from outside stays.
    pub fn reset(&mut self, context: &mut Context) {
        let supply_volts = self.supply_volts;
        *self = Saadc::new();
        self.supply_volts = supply_volts;
        context.cancel(event_key(ID, SAMPLE_TIMER_TOKEN));
    }

    /// Sets VDD, which is both an input and the VDD/4 reference.
    pub fn set_supply_volts(&mut self, volts: f64) {
        self.supply_volts = volts;
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    // ========================================================================
    // Registers
    // ========================================================================

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            // Conversions finish at once, so the converter is never busy.
            STATUS => 0,
            ENABLE => self.enabled as u32,
            RESULT_AMOUNT => self.result_amount,
            0x000..0x100 => 0,
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_START if triggers(value) && self.enabled => self.start(context),
            TASKS_SAMPLE if triggers(value) && self.enabled => self.sample_task(context),
            TASKS_STOP if triggers(value) => self.stop(context),
            TASKS_CALIBRATEOFFSET if triggers(value) => self.events.raise(EVENT_CALIBRATEDONE, context),
            0x000..0x100 | STATUS | RESULT_AMOUNT => {}
            ENABLE => {
                self.enabled = value & 1 != 0;
                if !self.enabled {
                    self.stop_sample_timer(context);
                }
            }
            _ => self.plain.write(offset, value),
        }
    }

    // ========================================================================
    // Tasks
    // ========================================================================

    // The pointer and count are latched here, so firmware may queue the next buffer straight away.
    fn start(&mut self, context: &mut Context) {
        self.buffer_pointer = self.plain.read(RESULT_PTR);
        self.buffer_maximum_count = self.plain.read(RESULT_MAXCNT);
        self.result_amount = 0;
        self.buffer_active = true;
        self.events.raise(EVENT_STARTED, context);
    }

    // In timer mode one SAMPLE task starts continuous sampling at 16 MHz / CC.
    fn sample_task(&mut self, context: &mut Context) {
        self.take_sample(context);
        let sample_rate = self.plain.read(SAMPLERATE);
        if sample_rate & (1 << 12) != 0 && !self.sample_timer_running {
            let compare = ((sample_rate & 0x7FF) as u64).max(SAMPLE_RATE_COMPARE_MINIMUM);
            self.sample_period = compare * CYCLES_PER_SAMPLE_CLOCK_TICK;
            self.sample_timer_running = true;
            context.schedule(
                event_key(ID, SAMPLE_TIMER_TOKEN),
                context.now + self.sample_period,
            );
        }
    }

    fn stop(&mut self, context: &mut Context) {
        self.stop_sample_timer(context);
        self.buffer_active = false;
        self.oversample_progress = 0;
        self.events.raise(EVENT_STOPPED, context);
    }

    fn stop_sample_timer(&mut self, context: &mut Context) {
        self.sample_timer_running = false;
        context.cancel(event_key(ID, SAMPLE_TIMER_TOKEN));
    }

    pub fn handle_event(&mut self, token: u32, context: &mut Context) {
        if token != SAMPLE_TIMER_TOKEN || !self.sample_timer_running {
            return;
        }
        context.schedule(
            event_key(ID, SAMPLE_TIMER_TOKEN),
            context.now + self.sample_period,
        );
        self.take_sample(context);
    }

    // ========================================================================
    // Conversion
    // ========================================================================

    fn channel_register(&self, channel: usize, register: u32) -> u32 {
        self.plain.read(CH + CH_STRIDE * channel as u32 + register)
    }

    fn take_sample(&mut self, context: &mut Context) {
        let active_channels: Vec<usize> = (0..CHANNEL_COUNT)
            .filter(|&channel| self.channel_register(channel, 0) & 0x1F != PSEL_NOT_CONNECTED)
            .collect();
        let Some(&first_channel) = active_channels.first() else {
            return;
        };

        // Oversampling is only defined for one channel; with more it is ignored.
        let oversample = self.plain.read(OVERSAMPLE);
        let mut samples_per_result = if oversample <= MAXIMUM_OVERSAMPLE {
            1 << oversample
        } else {
            1
        };
        if active_channels.len() > 1 {
            samples_per_result = 1;
        }

        // Without burst, an oversampled result needs one SAMPLE task per sample.
        self.events.raise(EVENT_DONE, context);
        let burst = self.channel_register(first_channel, 8) & (1 << 24) != 0;
        if samples_per_result > 1 && !burst {
            self.oversample_progress += 1;
            if self.oversample_progress < samples_per_result {
                return;
            }
        }
        self.oversample_progress = 0;

        for channel in active_channels {
            let result = self.convert(channel, context);
            self.store_result(result, context);
            self.check_limits(channel, result, context);
        }
    }

    // RESULT = (V(P) - V(N)) * GAIN / REFERENCE * 2^(RESOLUTION - m), where m is 1 for differential and 0 for single-ended.
    fn convert(&self, channel: usize, context: &Context) -> i16 {
        let configuration = self.channel_register(channel, 8);
        let differential = configuration & (1 << 20) != 0;
        let positive_volts = self.input_volts(self.channel_register(channel, 0) & 0x1F, context);
        let negative_volts = if differential {
            self.input_volts(self.channel_register(channel, 4) & 0x1F, context)
        } else {
            0.0
        };
        let gain = GAIN_FACTORS[((configuration >> 8) & 7) as usize];
        let reference_volts = if configuration & (1 << 12) != 0 {
            self.supply_volts / 4.0
        } else {
            INTERNAL_REFERENCE_VOLTS
        };
        let resolution_bits = match self.plain.read(RESOLUTION) {
            0 => 8,
            2 => 12,
            3 => 14,
            _ => 10,
        };
        let scale_bits = if differential {
            resolution_bits - 1
        } else {
            resolution_bits
        };

        let ideal = (positive_volts - negative_volts) * gain / reference_volts * (1u64 << scale_bits) as f64;
        // Single-ended inputs cannot go below ground; differential results are signed.
        let minimum = if differential { -(1i64 << scale_bits) } else { 0 };
        let maximum = (1i64 << scale_bits) - 1;
        (ideal.floor() as i64).clamp(minimum, maximum) as i16
    }

    fn input_volts(&self, input_select: u32, context: &Context) -> f64 {
        match input_select {
            PSEL_NOT_CONNECTED => 0.0,
            PSEL_VDD => self.supply_volts,
            1..=8 => context
                .analog_input_volts((input_select - 1) as u8)
                .unwrap_or(0.0) as f64,
            _ => 0.0,
        }
    }

    fn store_result(&mut self, result: i16, context: &mut Context) {
        if !self.buffer_active {
            return;
        }
        let address = self.buffer_pointer.wrapping_add(2 * self.result_amount);
        context.memory.dma_write(address, &result.to_le_bytes());
        self.result_amount += 1;
        self.events.raise(EVENT_RESULTDONE, context);
        if self.result_amount >= self.buffer_maximum_count {
            self.buffer_active = false;
            self.events.raise(EVENT_END, context);
        }
    }

    fn check_limits(&mut self, channel: usize, result: i16, context: &mut Context) {
        let limits = self.channel_register(channel, 12);
        let low = limits as u16 as i16;
        let high = (limits >> 16) as u16 as i16;
        if result > high {
            self.events.raise(EVENT_CH0_LIMITH + 2 * channel as u32, context);
        }
        if result < low {
            self.events.raise(EVENT_CH0_LIMITL + 2 * channel as u32, context);
        }
    }
}
