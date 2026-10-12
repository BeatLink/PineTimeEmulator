// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime's Tianyi HRS3300 heart rate sensor: a synthetic pulse and steady ambient light.

use std::f64::consts::PI;

use pinetime_core::{Cycle, DeviceContext, CPU_HERTZ};

use crate::i2c::I2cDevice;

// ============================================================================
// Registers and constants
// ============================================================================

const ID: u8 = 0x00;
const ENABLE: u8 = 0x01;
const C1DATAM: u8 = 0x08;
const C0DATAM: u8 = 0x09;
const C0DATAH: u8 = 0x0A;
const PDRIVER: u8 = 0x0C;
const C1DATAH: u8 = 0x0D;
const C1DATAL: u8 = 0x0E;
const C0DATAL: u8 = 0x0F;
const RES: u8 = 0x16;
const HGAIN: u8 = 0x17;

const ID_VALUE: u8 = 0x21;
const ENABLE_BIT: u8 = 0x80;
const DEFAULT_HEART_RATE: u32 = 70;
const DEFAULT_AMBIENT_LIGHT: u32 = 40;
const MAXIMUM_AMBIENT_LIGHT: u32 = (1 << 17) - 1;
const MINIMUM_CONVERSION_MILLISECONDS: f64 = 5.0;
const WAIT_TIME_MILLISECONDS: [f64; 8] = [800.0, 400.0, 200.0, 100.0, 75.0, 50.0, 12.5, 0.0];
const GAIN_FACTORS: [f64; 8] = [1.0, 2.0, 4.0, 8.0, 64.0, 64.0, 64.0, 64.0];

#[derive(Clone, Copy, Default)]
struct Sample {
    heart_rate_channel: u32,
    ambient_light_channel: u32,
}

fn seconds_at(cycle: Cycle) -> f64 {
    cycle as f64 / CPU_HERTZ as f64
}

// ============================================================================
// Sensor state
// ============================================================================

pub struct HeartRateSensor {
    registers: [u8; 256],
    register_pointer: u8,
    expecting_register_address: bool,
    heart_rate: u32,
    ambient_light: u32,
    phase_at_change: f64,
    time_of_change: Cycle,
    latched_sample_index: Option<u64>,
    latched_sample: Sample,
    /// The steady level of the pulse signal, before gain.
    pub pulse_baseline: i32,
    /// The height of each beat above and below the baseline, before gain.
    pub pulse_amplitude: i32,
}

impl Default for HeartRateSensor {
    fn default() -> HeartRateSensor {
        HeartRateSensor::new()
    }
}

impl HeartRateSensor {
    pub fn new() -> HeartRateSensor {
        let mut sensor = HeartRateSensor {
            registers: [0; 256],
            register_pointer: 0,
            expecting_register_address: false,
            heart_rate: DEFAULT_HEART_RATE,
            ambient_light: DEFAULT_AMBIENT_LIGHT,
            phase_at_change: 0.0,
            time_of_change: 0,
            latched_sample_index: None,
            latched_sample: Sample::default(),
            pulse_baseline: 9000,
            pulse_amplitude: 10,
        };
        sensor.reset(0);
        sensor
    }

    pub fn reset(&mut self, now: Cycle) {
        self.registers = [0; 256];
        self.registers[ID as usize] = ID_VALUE;
        self.registers[ENABLE as usize] = 0x68;
        self.registers[PDRIVER as usize] = 0x68;
        self.registers[RES as usize] = 0x66;
        self.registers[HGAIN as usize] = 0x10;
        self.register_pointer = 0;
        self.phase_at_change = 0.0;
        self.time_of_change = now;
        self.latched_sample_index = None;
    }

    // ========================================================================
    // Inputs
    // ========================================================================

    /// Sets the pulse in beats per minute; zero gives a flat signal, as with no finger on it.
    pub fn set_heart_rate(&mut self, beats_per_minute: u32, now: Cycle) {
        self.phase_at_change = self.phase_at(now);
        self.time_of_change = now;
        self.heart_rate = beats_per_minute;
    }

    pub fn heart_rate(&self) -> u32 {
        self.heart_rate
    }

    pub fn set_ambient_light(&mut self, level: u32) {
        self.ambient_light = level.min(MAXIMUM_AMBIENT_LIGHT);
    }

    // ========================================================================
    // Registers
    // ========================================================================

    fn write_register(&mut self, address: u8, value: u8) {
        match address {
            ID | C1DATAM | C0DATAM | C0DATAH | C1DATAH | C1DATAL | C0DATAL => {}
            _ => self.registers[address as usize] = value,
        }
    }

    /// The data bits are spread over six registers the way the datasheet lays them out.
    fn read_register(&mut self, address: u8, now: Cycle) -> u8 {
        match address {
            C0DATAM => (self.current_sample(now).heart_rate_channel >> 8) as u8,
            C0DATAH => ((self.current_sample(now).heart_rate_channel >> 4) & 0x0F) as u8,
            C0DATAL => {
                let value = self.current_sample(now).heart_rate_channel;
                ((value & 0x0F) | ((value >> 12) & 0x30)) as u8
            }
            C1DATAM => (self.current_sample(now).ambient_light_channel >> 3) as u8,
            C1DATAH => ((self.current_sample(now).ambient_light_channel >> 11) & 0x3F) as u8,
            C1DATAL => (self.current_sample(now).ambient_light_channel & 0x07) as u8,
            _ => self.registers[address as usize],
        }
    }

    // ========================================================================
    // Sampling
    // ========================================================================

    fn enabled(&self) -> bool {
        self.registers[ENABLE as usize] & ENABLE_BIT != 0
    }

    /// One conversion lasts the wait time chosen in the enable register, so reads within it agree.
    fn conversion_milliseconds(&self) -> f64 {
        let wait = WAIT_TIME_MILLISECONDS[((self.registers[ENABLE as usize] >> 4) & 0x07) as usize];
        wait.max(MINIMUM_CONVERSION_MILLISECONDS)
    }

    fn current_sample(&mut self, now: Cycle) -> Sample {
        if !self.enabled() {
            return Sample::default();
        }
        let period = self.conversion_milliseconds();
        let sample_index = (seconds_at(now) * 1000.0 / period) as u64;
        if self.latched_sample_index != Some(sample_index) {
            self.latched_sample_index = Some(sample_index);
            let sample_seconds = sample_index as f64 * period / 1000.0;
            self.latched_sample = Sample {
                heart_rate_channel: self.pulse_at(sample_seconds),
                ambient_light_channel: self.ambient_light,
            };
        }
        self.latched_sample
    }

    /// A beat is a smooth rise and fall with a small second hump, like a real pulse wave.
    fn pulse_at(&self, seconds: f64) -> u32 {
        let phase = 2.0 * PI * self.phase_at_seconds(seconds);
        let shape = if self.heart_rate == 0 {
            0.0
        } else {
            phase.sin() + 0.1 * (2.0 * phase + PI / 4.0).sin()
        };
        let value = (self.pulse_baseline as f64 + self.pulse_amplitude as f64 * shape) * self.gain_factor();
        let limit = ((1u32 << self.heart_rate_bits()) - 1) as f64;
        value.round_ties_even().clamp(0.0, limit) as u32
    }

    fn phase_at(&self, now: Cycle) -> f64 {
        self.phase_at_seconds(seconds_at(now))
    }

    /// The number of beats since the last rate change, plus the beats before it.
    fn phase_at_seconds(&self, seconds: f64) -> f64 {
        let elapsed = seconds - seconds_at(self.time_of_change);
        self.phase_at_change + elapsed.max(0.0) * self.heart_rate as f64 / 60.0
    }

    fn gain_factor(&self) -> f64 {
        GAIN_FACTORS[((self.registers[HGAIN as usize] >> 2) & 0x07) as usize]
    }

    fn heart_rate_bits(&self) -> u32 {
        (8 + (self.registers[RES as usize] & 0x0F) as u32).min(18)
    }
}

impl I2cDevice for HeartRateSensor {
    fn start(&mut self, read: bool, _context: &mut dyn DeviceContext) {
        if !read {
            self.expecting_register_address = true;
        }
    }

    fn write(&mut self, byte: u8, _context: &mut dyn DeviceContext) {
        if self.expecting_register_address {
            self.expecting_register_address = false;
            self.register_pointer = byte;
            return;
        }
        self.write_register(self.register_pointer, byte);
        self.register_pointer = self.register_pointer.wrapping_add(1);
    }

    fn read(&mut self, context: &mut dyn DeviceContext) -> u8 {
        let value = self.read_register(self.register_pointer, context.now());
        self.register_pointer = self.register_pointer.wrapping_add(1);
        value
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{read_register, read_registers, write_bytes, TestContext};

    fn data_should_be_zero(sensor: &mut HeartRateSensor, context: &mut TestContext) {
        assert_eq!(read_registers(sensor, C1DATAM, 3, context), vec![0, 0, 0]);
        assert_eq!(read_registers(sensor, C1DATAH, 3, context), vec![0, 0, 0]);
    }

    #[test]
    fn reports_its_id() {
        let mut context = TestContext::new();
        let mut sensor = HeartRateSensor::new();
        assert_eq!(read_register(&mut sensor, ID, &mut context), 0x21);
    }

    #[test]
    fn gives_zero_while_disabled() {
        let mut context = TestContext::new();
        let mut sensor = HeartRateSensor::new();
        write_bytes(&mut sensor, &[ENABLE, 0x60], &mut context);
        data_should_be_zero(&mut sensor, &mut context);
    }

    #[test]
    fn pulse_changes_over_virtual_time_when_enabled() {
        let mut context = TestContext::new();
        let mut sensor = HeartRateSensor::new();
        write_bytes(&mut sensor, &[RES, 0x88], &mut context);
        write_bytes(&mut sensor, &[HGAIN, 0x00], &mut context);
        sensor.set_heart_rate(60, context.now);
        sensor.set_ambient_light(1234);
        write_bytes(&mut sensor, &[ENABLE, 0xE0], &mut context);
        let first = read_registers(&mut sensor, C1DATAM, 8, &mut context);
        context.advance_milliseconds(250, |_, _| {});
        let second = read_registers(&mut sensor, C1DATAM, 8, &mut context);
        assert_ne!(first, second);

        assert_eq!(read_register(&mut sensor, C1DATAM, &mut context), 0x9A);
        assert_eq!(
            read_registers(&mut sensor, C1DATAH, 2, &mut context),
            vec![0x00, 0x02]
        );
        write_bytes(&mut sensor, &[ENABLE, 0x60], &mut context);
        data_should_be_zero(&mut sensor, &mut context);
    }

    #[test]
    fn reads_within_one_conversion_agree() {
        let mut context = TestContext::new();
        let mut sensor = HeartRateSensor::new();
        write_bytes(&mut sensor, &[ENABLE, 0x80], &mut context);
        context.now = 1_000;
        let first = read_registers(&mut sensor, C0DATAM, 2, &mut context);
        context.now = 2_000;
        assert_eq!(first, read_registers(&mut sensor, C0DATAM, 2, &mut context));
    }

    #[test]
    fn a_zero_heart_rate_gives_the_flat_baseline_times_gain() {
        let mut context = TestContext::new();
        let mut sensor = HeartRateSensor::new();
        write_bytes(&mut sensor, &[RES, 0x08], &mut context);
        write_bytes(&mut sensor, &[HGAIN, 0x04], &mut context);
        sensor.set_heart_rate(0, 0);
        write_bytes(&mut sensor, &[ENABLE, 0x80], &mut context);
        let middle = read_register(&mut sensor, C0DATAM, &mut context) as u32;
        let high = read_register(&mut sensor, C0DATAH, &mut context) as u32;
        let low = read_register(&mut sensor, C0DATAL, &mut context) as u32;
        let value = (middle << 8) | (high << 4) | (low & 0x0F) | ((low & 0x30) << 12);
        assert_eq!(value, 18_000);
        assert_eq!(sensor.heart_rate(), 0);
    }

    #[test]
    fn data_registers_ignore_writes() {
        let mut context = TestContext::new();
        let mut sensor = HeartRateSensor::new();
        write_bytes(&mut sensor, &[ID, 0x99], &mut context);
        assert_eq!(read_register(&mut sensor, ID, &mut context), 0x21);
        write_bytes(&mut sensor, &[PDRIVER, 0x2F], &mut context);
        assert_eq!(read_register(&mut sensor, PDRIVER, &mut context), 0x2F);
    }
}
