// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime's Bosch BMA421 or BMA425 accelerometer: config upload, acceleration and a step counter.

use std::collections::VecDeque;

use pinetime_core::time::cycles_from_milliseconds;
use pinetime_core::{Cycle, DeviceContext, EventKey, Owner, PinChange};

use crate::board::{ACCELEROMETER_DEVICE, ACCELEROMETER_INTERRUPT_PIN};
use crate::i2c::I2cDevice;

// ============================================================================
// Registers and constants
// ============================================================================

const CHIP_ID: u8 = 0x00;
const STATUS: u8 = 0x03;
const DATA_8: u8 = 0x12;
const DATA_13: u8 = 0x17;
const SENSORTIME_0: u8 = 0x18;
const SENSORTIME_1: u8 = 0x19;
const SENSORTIME_2: u8 = 0x1A;
const EVENT: u8 = 0x1B;
const INT_STATUS_0: u8 = 0x1C;
const INT_STATUS_1: u8 = 0x1D;
const STEP_COUNTER_0: u8 = 0x1E;
const STEP_COUNTER_1: u8 = 0x1F;
const STEP_COUNTER_2: u8 = 0x20;
const STEP_COUNTER_3: u8 = 0x21;
const TEMPERATURE: u8 = 0x22;
const FIFO_LENGTH_0: u8 = 0x24;
const FIFO_LENGTH_1: u8 = 0x25;
const FIFO_DATA: u8 = 0x26;
const INTERNAL_STATUS: u8 = 0x2A;
const ACC_CONF: u8 = 0x40;
const ACC_RANGE: u8 = 0x41;
const FIFO_CONFIG_1: u8 = 0x49;
const INT1_IO_CTRL: u8 = 0x53;
const INT2_IO_CTRL: u8 = 0x54;
const INT_LATCH: u8 = 0x55;
const INT1_MAP: u8 = 0x56;
const INT2_MAP: u8 = 0x57;
const INT_MAP_DATA: u8 = 0x58;
const INIT_CTRL: u8 = 0x59;
const RESERVED_REG_5B: u8 = 0x5B;
const RESERVED_REG_5C: u8 = 0x5C;
const FEATURES_IN: u8 = 0x5E;
const PWR_CONF: u8 = 0x7C;
const PWR_CTRL: u8 = 0x7D;
const CMD: u8 = 0x7E;

const BMA421_CHIP_ID: u8 = 0x11;
const BMA425_CHIP_ID: u8 = 0x13;

const REGISTER_COUNT: usize = 128;
const CONFIG_MEMORY_SIZE: usize = 8192;
const FEATURE_MEMORY_SIZE: usize = 256;

const SOFT_RESET_COMMAND: u8 = 0xB6;
const FIFO_FLUSH_COMMAND: u8 = 0xB0;

const COMMAND_READY: u8 = 0x10;
const DATA_READY: u8 = 0x80;
const POWER_ON_DETECTED: u8 = 0x01;
const ACCELEROMETER_ENABLE_BIT: u8 = 0x04;

const INTERNAL_STATUS_NOT_INITIALISED: u8 = 0x00;
const INTERNAL_STATUS_INITIALISED: u8 = 0x01;
const INTERNAL_STATUS_INITIALISATION_ERROR: u8 = 0x02;

const STEP_COUNTER_CONTROL_OFFSET: usize = 0x3B;
const STEP_COUNTER_RESET_BIT: u8 = 0x04;
pub const STEP_COUNTER_INTERRUPT: u8 = 0x02;
const UNLATCHED_PULSE_MILLISECONDS: u64 = 1;

/// Sensor time ticks every 39.0625 microseconds, which is 2500 CPU cycles.
const CYCLES_PER_SENSOR_TIME_TICK: u64 = 2500;

const TOKEN_UNLATCHED_PULSE_END: u32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccelerometerModel {
    Bma421,
    Bma425,
}

impl AccelerometerModel {
    /// Reads "BMA421" or "BMA425", in any case.
    pub fn parse(text: &str) -> Option<AccelerometerModel> {
        match text.trim().to_ascii_uppercase().as_str() {
            "BMA421" => Some(AccelerometerModel::Bma421),
            "BMA425" => Some(AccelerometerModel::Bma425),
            _ => None,
        }
    }

    pub fn chip_id(self) -> u8 {
        match self {
            AccelerometerModel::Bma421 => BMA421_CHIP_ID,
            AccelerometerModel::Bma425 => BMA425_CHIP_ID,
        }
    }
}

fn pulse_end_key() -> EventKey {
    EventKey::new(Owner::Device(ACCELEROMETER_DEVICE), TOKEN_UNLATCHED_PULSE_END)
}

// ============================================================================
// Sensor state
// ============================================================================

pub struct Accelerometer {
    model: AccelerometerModel,
    registers: [u8; REGISTER_COUNT],
    config_memory: Box<[u8]>,
    feature_memory: [u8; FEATURE_MEMORY_SIZE],
    acceleration_milli_g: [i32; 3],
    temperature_celsius: f64,
    register_pointer: u8,
    expecting_register_address: bool,
    feature_offset: usize,
    initialised: bool,
    config_bytes_loaded: usize,
    step_count: u32,
    interrupt_1_level: Option<bool>,
    interrupt_2_level: bool,
    unlatched_pulses: VecDeque<(Cycle, u8)>,
}

impl Accelerometer {
    pub fn new(model: AccelerometerModel) -> Accelerometer {
        let mut accelerometer = Accelerometer {
            model,
            registers: [0; REGISTER_COUNT],
            config_memory: vec![0; CONFIG_MEMORY_SIZE].into_boxed_slice(),
            feature_memory: [0; FEATURE_MEMORY_SIZE],
            acceleration_milli_g: [0, 0, 1000],
            temperature_celsius: 23.0,
            register_pointer: 0,
            expecting_register_address: false,
            feature_offset: 0,
            initialised: false,
            config_bytes_loaded: 0,
            step_count: 0,
            interrupt_1_level: None,
            interrupt_2_level: false,
            unlatched_pulses: VecDeque::new(),
        };
        accelerometer.restore_defaults();
        accelerometer
    }

    pub fn model(&self) -> AccelerometerModel {
        self.model
    }

    /// Soft-resets the sensor and drives its interrupt line low.
    pub fn reset(&mut self, context: &mut dyn DeviceContext) {
        self.soft_reset(context);
    }

    // ========================================================================
    // Inputs
    // ========================================================================

    /// Sets the acceleration the sensor feels, in milli-g along its own axes.
    pub fn set_acceleration(&mut self, x: i32, y: i32, z: i32) {
        self.acceleration_milli_g = [x, y, z];
    }

    pub fn acceleration_milli_g(&self) -> [i32; 3] {
        self.acceleration_milli_g
    }

    pub fn set_steps(&mut self, steps: u32, context: &mut dyn DeviceContext) {
        let before = self.step_count;
        self.step_count = steps;
        self.check_step_watermark(before, context);
    }

    pub fn add_steps(&mut self, steps: u32, context: &mut dyn DeviceContext) {
        let before = self.step_count;
        self.step_count = self.step_count.wrapping_add(steps);
        self.check_step_watermark(before, context);
    }

    pub fn steps(&self) -> u32 {
        self.step_count
    }

    pub fn set_temperature(&mut self, celsius: f64) {
        self.temperature_celsius = celsius;
    }

    /// Raises feature interrupts by their INT_STATUS_0 bits, such as 0x02 for the step counter.
    pub fn trigger_feature_interrupt(&mut self, status_bits: u8, context: &mut dyn DeviceContext) {
        self.raise_feature_interrupt(status_bits, context);
    }

    /// The level the sensor drives on INT1.
    pub fn interrupt_1_high(&self) -> bool {
        self.interrupt_1_level.unwrap_or(false)
    }

    /// INT2 is not wired on the PineTime; its level is kept for tests.
    pub fn interrupt_2_high(&self) -> bool {
        self.interrupt_2_level
    }

    pub fn handle_event(&mut self, token: u32, context: &mut dyn DeviceContext) {
        if token != TOKEN_UNLATCHED_PULSE_END {
            return;
        }
        let now = context.now();
        while let Some((due, status_bits)) = self.unlatched_pulses.front().copied() {
            if due > now {
                context.schedule(pulse_end_key(), due);
                break;
            }
            self.unlatched_pulses.pop_front();
            self.registers[INT_STATUS_0 as usize] &= !status_bits;
            self.update_interrupt_pins(context);
        }
    }

    // ========================================================================
    // Registers
    // ========================================================================

    fn select_register(&mut self, address: u8) {
        self.register_pointer = address;
        if address == FEATURES_IN {
            self.feature_offset = self.feature_word_address() * 2;
        }
    }

    /// The pointer moves on after each byte, except on the FIFO port and on the feature port.
    fn advance_pointer(&mut self) {
        if self.register_pointer == FEATURES_IN {
            self.feature_offset += 1;
        } else if self.register_pointer != FIFO_DATA {
            self.register_pointer = (self.register_pointer + 1) & 0x7F;
        }
    }

    fn read_register(&mut self, address: u8, context: &mut dyn DeviceContext) -> u8 {
        match address {
            CHIP_ID => self.model.chip_id(),
            STATUS => {
                COMMAND_READY
                    | if self.accelerometer_enabled() {
                        DATA_READY
                    } else {
                        0
                    }
            }
            DATA_8..=DATA_13 => self.acceleration_byte((address - DATA_8) as usize),
            SENSORTIME_0 => sensor_time_ticks(context.now()) as u8,
            SENSORTIME_1 => (sensor_time_ticks(context.now()) >> 8) as u8,
            SENSORTIME_2 => (sensor_time_ticks(context.now()) >> 16) as u8,
            EVENT => std::mem::take(&mut self.registers[address as usize]),
            INT_STATUS_0 | INT_STATUS_1 => {
                let value = std::mem::take(&mut self.registers[address as usize]);
                self.update_interrupt_pins(context);
                value
            }
            STEP_COUNTER_0 => self.step_count as u8,
            STEP_COUNTER_1 => (self.step_count >> 8) as u8,
            STEP_COUNTER_2 => (self.step_count >> 16) as u8,
            STEP_COUNTER_3 => (self.step_count >> 24) as u8,
            TEMPERATURE => {
                let offset = (self.temperature_celsius - 23.0)
                    .round_ties_even()
                    .clamp(-127.0, 127.0);
                offset as i8 as u8
            }
            FIFO_LENGTH_0 | FIFO_LENGTH_1 | FIFO_DATA => 0,
            FEATURES_IN => self.feature_port_read(),
            _ => self.registers.get(address as usize).copied().unwrap_or(0),
        }
    }

    fn write_register(&mut self, address: u8, value: u8, context: &mut dyn DeviceContext) {
        match address {
            CMD => self.run_command(value, context),
            INIT_CTRL => {
                self.registers[address as usize] = value;
                if value == 0 {
                    self.initialised = false;
                    self.registers[INTERNAL_STATUS as usize] = INTERNAL_STATUS_NOT_INITIALISED;
                } else if value & 0x01 != 0 {
                    self.finish_config_load();
                }
            }
            FEATURES_IN => self.feature_port_write(value),
            CHIP_ID | STATUS => {}
            DATA_8..=INTERNAL_STATUS => {}
            INT_LATCH | INT1_IO_CTRL | INT2_IO_CTRL | INT1_MAP | INT2_MAP | INT_MAP_DATA => {
                self.registers[address as usize] = value;
                self.update_interrupt_pins(context);
            }
            _ => {
                if let Some(register) = self.registers.get_mut(address as usize) {
                    *register = value;
                }
            }
        }
    }

    fn run_command(&mut self, command: u8, context: &mut dyn DeviceContext) {
        match command {
            SOFT_RESET_COMMAND => self.soft_reset(context),
            FIFO_FLUSH_COMMAND => {}
            _ => {}
        }
    }

    fn soft_reset(&mut self, context: &mut dyn DeviceContext) {
        self.restore_defaults();
        self.unlatched_pulses.clear();
        context.cancel(pulse_end_key());
        self.interrupt_2_level = false;
        self.set_interrupt_1(false, context);
    }

    fn restore_defaults(&mut self) {
        self.registers = [0; REGISTER_COUNT];
        self.config_memory.fill(0);
        self.feature_memory = [0; FEATURE_MEMORY_SIZE];
        self.register_pointer = 0;
        self.feature_offset = 0;
        self.initialised = false;
        self.config_bytes_loaded = 0;
        self.step_count = 0;

        self.registers[EVENT as usize] = POWER_ON_DETECTED;
        self.registers[INTERNAL_STATUS as usize] = INTERNAL_STATUS_NOT_INITIALISED;
        self.registers[ACC_CONF as usize] = 0xA8;
        self.registers[ACC_RANGE as usize] = 0x01;
        self.registers[FIFO_CONFIG_1 as usize] = 0x10;
        self.registers[INIT_CTRL as usize] = 0x90;
        self.registers[PWR_CONF as usize] = 0x03;
    }

    // ========================================================================
    // Config memory and feature area
    // ========================================================================

    /// The word address set through registers 0x5B (low four bits) and 0x5C (the rest).
    fn feature_word_address(&self) -> usize {
        ((self.registers[RESERVED_REG_5C as usize] as usize) << 4)
            | (self.registers[RESERVED_REG_5B as usize] & 0x0F) as usize
    }

    fn feature_port_write(&mut self, value: u8) {
        if self.initialised {
            if self.feature_offset < FEATURE_MEMORY_SIZE {
                self.feature_memory[self.feature_offset] = value;
                if self.feature_offset == STEP_COUNTER_CONTROL_OFFSET {
                    self.apply_step_counter_control();
                }
            }
            return;
        }
        if self.feature_offset < CONFIG_MEMORY_SIZE {
            self.config_memory[self.feature_offset] = value;
            self.config_bytes_loaded += 1;
        }
    }

    fn feature_port_read(&self) -> u8 {
        let memory: &[u8] = if self.initialised {
            &self.feature_memory
        } else {
            &self.config_memory
        };
        memory.get(self.feature_offset).copied().unwrap_or(0)
    }

    /// Starting the loaded config makes the feature area appear at word address zero.
    fn finish_config_load(&mut self) {
        if self.config_bytes_loaded == 0 {
            self.registers[INTERNAL_STATUS as usize] = INTERNAL_STATUS_INITIALISATION_ERROR;
            return;
        }
        self.initialised = true;
        self.registers[INTERNAL_STATUS as usize] = INTERNAL_STATUS_INITIALISED;
        self.registers[RESERVED_REG_5B as usize] = 0;
        self.registers[RESERVED_REG_5C as usize] = 0;
        self.feature_memory = [0; FEATURE_MEMORY_SIZE];
    }

    /// The step counter's reset bit clears the count and then clears itself.
    fn apply_step_counter_control(&mut self) {
        if self.feature_memory[STEP_COUNTER_CONTROL_OFFSET] & STEP_COUNTER_RESET_BIT != 0 {
            self.step_count = 0;
            self.feature_memory[STEP_COUNTER_CONTROL_OFFSET] &= !STEP_COUNTER_RESET_BIT;
        }
    }

    // ========================================================================
    // Measurements
    // ========================================================================

    fn accelerometer_enabled(&self) -> bool {
        self.registers[PWR_CTRL as usize] & ACCELEROMETER_ENABLE_BIT != 0
    }

    /// Acceleration as 12-bit counts, left-justified in 16 bits, low byte first.
    fn acceleration_byte(&self, index: usize) -> u8 {
        let counts_per_g = 1024 >> (self.registers[ACC_RANGE as usize] & 0x03);
        let milli_g = self.acceleration_milli_g[index / 2] as f64;
        let counts = (milli_g * counts_per_g as f64 / 1000.0)
            .round_ties_even()
            .clamp(-2048.0, 2047.0) as i32;
        let left_justified = ((counts << 4) as i16) as u16;
        if index.is_multiple_of(2) {
            left_justified as u8
        } else {
            (left_justified >> 8) as u8
        }
    }

    // ========================================================================
    // Interrupts
    // ========================================================================

    fn check_step_watermark(&mut self, before: u32, context: &mut dyn DeviceContext) {
        let control = self.feature_memory[STEP_COUNTER_CONTROL_OFFSET - 1] as u32
            | ((self.feature_memory[STEP_COUNTER_CONTROL_OFFSET] as u32) << 8);
        let watermark = (control & 0x3FF) * 20;
        if watermark != 0 && self.step_count / watermark != before / watermark {
            self.raise_feature_interrupt(STEP_COUNTER_INTERRUPT, context);
        }
    }

    /// Without latching, the status bits and the pins only hold for a moment.
    fn raise_feature_interrupt(&mut self, status_bits: u8, context: &mut dyn DeviceContext) {
        self.registers[INT_STATUS_0 as usize] |= status_bits;
        let latched = self.registers[INT_LATCH as usize] & 0x01 != 0;
        self.update_interrupt_pins(context);
        if !latched {
            let due = context.now() + cycles_from_milliseconds(UNLATCHED_PULSE_MILLISECONDS);
            self.unlatched_pulses.push_back((due, status_bits));
            if let Some((first_due, _)) = self.unlatched_pulses.front() {
                context.schedule(pulse_end_key(), *first_due);
            }
        }
    }

    fn update_interrupt_pins(&mut self, context: &mut dyn DeviceContext) {
        let status = self.registers[INT_STATUS_0 as usize];
        let interrupt_1 = pin_level(
            status & self.registers[INT1_MAP as usize] != 0,
            self.registers[INT1_IO_CTRL as usize],
        );
        self.interrupt_2_level = pin_level(
            status & self.registers[INT2_MAP as usize] != 0,
            self.registers[INT2_IO_CTRL as usize],
        );
        self.set_interrupt_1(interrupt_1, context);
    }

    fn set_interrupt_1(&mut self, high: bool, context: &mut dyn DeviceContext) {
        if self.interrupt_1_level == Some(high) {
            return;
        }
        self.interrupt_1_level = Some(high);
        context.drive_pin(PinChange {
            pin: ACCELEROMETER_INTERRUPT_PIN,
            level: Some(high),
        });
    }
}

impl I2cDevice for Accelerometer {
    fn start(&mut self, read: bool, _context: &mut dyn DeviceContext) {
        if !read {
            self.expecting_register_address = true;
        }
    }

    fn write(&mut self, byte: u8, context: &mut dyn DeviceContext) {
        if self.expecting_register_address {
            self.expecting_register_address = false;
            self.select_register(byte);
            return;
        }
        self.write_register(self.register_pointer, byte, context);
        self.advance_pointer();
    }

    fn read(&mut self, context: &mut dyn DeviceContext) -> u8 {
        let value = self.read_register(self.register_pointer, context);
        self.advance_pointer();
        value
    }
}

/// A pin shows its configured active level, or stays low while its output is off.
fn pin_level(active: bool, io_control: u8) -> bool {
    let output_enabled = io_control & 0x08 != 0;
    let active_high = io_control & 0x02 != 0;
    output_enabled && (active == active_high)
}

fn sensor_time_ticks(now: Cycle) -> u32 {
    (now / CYCLES_PER_SENSOR_TIME_TICK) as u32 & 0xFF_FFFF
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{read_register, read_registers, write_bytes, TestContext};

    fn sensor(model: AccelerometerModel, context: &mut TestContext) -> Accelerometer {
        let mut accelerometer = Accelerometer::new(model);
        accelerometer.reset(context);
        accelerometer
    }

    fn write_register(accelerometer: &mut Accelerometer, register: u8, value: u8, context: &mut TestContext) {
        write_bytes(accelerometer, &[register, value], context);
    }

    fn load_bosch_config_file(accelerometer: &mut Accelerometer, context: &mut TestContext) {
        write_register(accelerometer, CMD, 0xB6, context);
        write_register(accelerometer, PWR_CONF, 0x02, context);
        write_register(accelerometer, INIT_CTRL, 0x00, context);
        write_register(accelerometer, RESERVED_REG_5B, 0x00, context);
        write_register(accelerometer, RESERVED_REG_5C, 0x00, context);
        write_bytes(
            accelerometer,
            &[
                0x5E, 0xC8, 0x2E, 0x00, 0x2E, 0x80, 0x2E, 0x3D, 0xB1, 0xC8, 0x2E, 0x00, 0x2E, 0x80, 0x2E,
                0x91, 0x03,
            ],
            context,
        );
        write_register(accelerometer, RESERVED_REG_5B, 0x08, context);
        write_register(accelerometer, RESERVED_REG_5C, 0x00, context);
        write_bytes(
            accelerometer,
            &[
                0x5E, 0x80, 0x2E, 0xBC, 0x03, 0x80, 0x2E, 0x8A, 0xB1, 0x80, 0x2E, 0x10, 0x00, 0x80, 0x2E,
                0x00, 0x2E,
            ],
            context,
        );
    }

    #[test]
    fn reports_the_chip_id_of_each_variant() {
        let mut context = TestContext::new();
        let mut bma421 = sensor(AccelerometerModel::Bma421, &mut context);
        let mut bma425 = sensor(AccelerometerModel::Bma425, &mut context);
        assert_eq!(read_register(&mut bma421, CHIP_ID, &mut context), 0x11);
        assert_eq!(read_register(&mut bma425, CHIP_ID, &mut context), 0x13);
        assert_eq!(
            AccelerometerModel::parse("bma425"),
            Some(AccelerometerModel::Bma425)
        );
        assert_eq!(AccelerometerModel::parse("BMA400"), None);
    }

    #[test]
    fn accepts_the_bosch_init_sequence() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        assert_eq!(
            read_register(&mut accelerometer, INTERNAL_STATUS, &mut context),
            0x00
        );
        load_bosch_config_file(&mut accelerometer, &mut context);

        write_register(&mut accelerometer, RESERVED_REG_5B, 0x08, &mut context);
        let chunk = read_registers(&mut accelerometer, FEATURES_IN, 4, &mut context);
        assert_eq!(chunk, vec![0x80, 0x2E, 0xBC, 0x03]);
        write_register(&mut accelerometer, INIT_CTRL, 0x01, &mut context);
        assert_eq!(
            read_register(&mut accelerometer, INTERNAL_STATUS, &mut context),
            0x01
        );
        assert_eq!(
            read_registers(&mut accelerometer, RESERVED_REG_5B, 2, &mut context),
            vec![0x00, 0x00]
        );
    }

    #[test]
    fn without_a_config_file_fails_to_start() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        write_register(&mut accelerometer, CMD, 0xB6, &mut context);
        write_register(&mut accelerometer, INIT_CTRL, 0x01, &mut context);
        assert_eq!(
            read_register(&mut accelerometer, INTERNAL_STATUS, &mut context),
            0x02
        );
    }

    #[test]
    fn feature_area_reads_back_what_was_written() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        load_bosch_config_file(&mut accelerometer, &mut context);
        write_register(&mut accelerometer, INIT_CTRL, 0x01, &mut context);
        write_bytes(&mut accelerometer, &[0x5E, 0x11, 0x22, 0x33, 0x44], &mut context);
        let features = read_registers(&mut accelerometer, FEATURES_IN, 4, &mut context);
        assert_eq!(features, vec![0x11, 0x22, 0x33, 0x44]);

        write_register(&mut accelerometer, RESERVED_REG_5B, 0x02, &mut context);
        assert_eq!(
            read_registers(&mut accelerometer, FEATURES_IN, 2, &mut context),
            vec![0x00, 0x00]
        );
        write_register(&mut accelerometer, RESERVED_REG_5B, 0x01, &mut context);
        assert_eq!(
            read_registers(&mut accelerometer, FEATURES_IN, 2, &mut context),
            vec![0x33, 0x44]
        );
    }

    #[test]
    fn reports_acceleration_in_the_chosen_range() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        write_register(&mut accelerometer, ACC_RANGE, 0x00, &mut context);
        accelerometer.set_acceleration(1000, -500, 250);
        let data = read_registers(&mut accelerometer, DATA_8, 6, &mut context);
        assert_eq!(data, vec![0x00, 0x40, 0x00, 0xE0, 0x00, 0x10]);
        write_register(&mut accelerometer, ACC_RANGE, 0x02, &mut context);
        assert_eq!(
            read_registers(&mut accelerometer, DATA_8, 2, &mut context),
            vec![0x00, 0x10]
        );
    }

    #[test]
    fn counts_steps() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        accelerometer.set_steps(70000, &mut context);
        let steps = read_registers(&mut accelerometer, STEP_COUNTER_0, 4, &mut context);
        assert_eq!(steps, vec![0x70, 0x11, 0x01, 0x00]);
        accelerometer.add_steps(5, &mut context);
        let steps = read_registers(&mut accelerometer, STEP_COUNTER_0, 4, &mut context);
        assert_eq!(steps, vec![0x75, 0x11, 0x01, 0x00]);
        assert_eq!(accelerometer.steps(), 70005);
    }

    #[test]
    fn step_counter_reset_bit_clears_the_count() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        load_bosch_config_file(&mut accelerometer, &mut context);
        write_register(&mut accelerometer, INIT_CTRL, 0x01, &mut context);
        accelerometer.set_steps(42, &mut context);
        write_register(&mut accelerometer, RESERVED_REG_5B, 0x0D, &mut context);
        write_register(&mut accelerometer, RESERVED_REG_5C, 0x01, &mut context);
        write_bytes(&mut accelerometer, &[0x5E, 0x00, 0x14], &mut context);
        let steps = read_registers(&mut accelerometer, STEP_COUNTER_0, 4, &mut context);
        assert_eq!(steps, vec![0x00, 0x00, 0x00, 0x00]);
        assert_eq!(
            read_registers(&mut accelerometer, FEATURES_IN, 2, &mut context),
            vec![0x00, 0x10]
        );
    }

    #[test]
    fn reports_temperature_and_sensor_time() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        accelerometer.set_temperature(30.0);
        assert_eq!(read_register(&mut accelerometer, TEMPERATURE, &mut context), 0x07);
        accelerometer.set_temperature(10.0);
        assert_eq!(
            read_register(&mut accelerometer, TEMPERATURE, &mut context),
            (-13i8) as u8
        );
        let before = read_registers(&mut accelerometer, SENSORTIME_0, 3, &mut context);
        context.advance_milliseconds(10, |_, _| {});
        let after = read_registers(&mut accelerometer, SENSORTIME_0, 3, &mut context);
        assert_ne!(before, after);
        assert_eq!(sensor_time_ticks(cycles_from_milliseconds(10)), 256);
    }

    #[test]
    fn status_shows_data_ready_once_enabled_and_event_clears_on_read() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma425, &mut context);
        assert_eq!(
            read_register(&mut accelerometer, STATUS, &mut context),
            COMMAND_READY
        );
        write_register(
            &mut accelerometer,
            PWR_CTRL,
            ACCELEROMETER_ENABLE_BIT,
            &mut context,
        );
        assert_eq!(
            read_register(&mut accelerometer, STATUS, &mut context),
            COMMAND_READY | DATA_READY
        );
        assert_eq!(
            read_register(&mut accelerometer, EVENT, &mut context),
            POWER_ON_DETECTED
        );
        assert_eq!(read_register(&mut accelerometer, EVENT, &mut context), 0);
    }

    #[test]
    fn a_step_watermark_pulses_int1_without_latching() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        assert_eq!(context.driven_level(ACCELEROMETER_INTERRUPT_PIN), Some(false));
        load_bosch_config_file(&mut accelerometer, &mut context);
        write_register(&mut accelerometer, INIT_CTRL, 0x01, &mut context);
        write_register(&mut accelerometer, INT1_IO_CTRL, 0x0A, &mut context);
        write_register(&mut accelerometer, INT1_MAP, STEP_COUNTER_INTERRUPT, &mut context);
        write_register(&mut accelerometer, RESERVED_REG_5B, 0x0D, &mut context);
        write_register(&mut accelerometer, RESERVED_REG_5C, 0x01, &mut context);
        write_bytes(&mut accelerometer, &[0x5E, 0x01, 0x00], &mut context);

        accelerometer.add_steps(19, &mut context);
        assert!(!accelerometer.interrupt_1_high());
        accelerometer.add_steps(1, &mut context);
        assert!(accelerometer.interrupt_1_high());
        assert_eq!(context.driven_level(ACCELEROMETER_INTERRUPT_PIN), Some(true));

        context.advance_milliseconds(1, |key, context| accelerometer.handle_event(key.token, context));

        assert!(!accelerometer.interrupt_1_high());
        assert_eq!(context.driven_level(ACCELEROMETER_INTERRUPT_PIN), Some(false));
    }

    #[test]
    fn a_latched_interrupt_holds_until_the_status_is_read() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        write_register(&mut accelerometer, INT_LATCH, 0x01, &mut context);
        write_register(&mut accelerometer, INT1_IO_CTRL, 0x0A, &mut context);
        write_register(&mut accelerometer, INT1_MAP, 0x40, &mut context);

        accelerometer.trigger_feature_interrupt(0x40, &mut context);
        context.advance_milliseconds(5, |key, context| accelerometer.handle_event(key.token, context));
        assert!(accelerometer.interrupt_1_high());

        assert_eq!(
            read_register(&mut accelerometer, INT_STATUS_0, &mut context),
            0x40
        );
        assert!(!accelerometer.interrupt_1_high());
    }

    #[test]
    fn an_active_low_pin_idles_high_once_its_output_is_on() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        write_register(&mut accelerometer, INT1_IO_CTRL, 0x08, &mut context);
        assert!(accelerometer.interrupt_1_high());
        write_register(&mut accelerometer, INT2_IO_CTRL, 0x08, &mut context);
        assert!(accelerometer.interrupt_2_high());
    }

    #[test]
    fn soft_reset_clears_steps_and_config() {
        let mut context = TestContext::new();
        let mut accelerometer = sensor(AccelerometerModel::Bma421, &mut context);
        load_bosch_config_file(&mut accelerometer, &mut context);
        write_register(&mut accelerometer, INIT_CTRL, 0x01, &mut context);
        accelerometer.set_steps(9, &mut context);
        write_register(&mut accelerometer, CMD, SOFT_RESET_COMMAND, &mut context);
        assert_eq!(accelerometer.steps(), 0);
        assert_eq!(read_register(&mut accelerometer, INIT_CTRL, &mut context), 0x90);
        assert_eq!(
            read_register(&mut accelerometer, INTERNAL_STATUS, &mut context),
            0x00
        );
    }
}
