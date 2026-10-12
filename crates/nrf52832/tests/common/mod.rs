// SPDX-License-Identifier: GPL-3.0-only
//! A fake board and a small machine loop for driving the chip in tests.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pinetime_core::{AccessSize, BoardPorts, Cycle, DeviceContext, EventKey, Owner, PinChange, Scheduler};
use pinetime_nrf52832::{ChipPorts, Nrf52832, RadioFrame, RadioMedium, ResetKind, Variant};

pub const CYCLES_PER_MICROSECOND: u64 = 64;

pub fn microseconds(value: f64) -> Cycle {
    (value * CYCLES_PER_MICROSECOND as f64).round() as Cycle
}

pub fn seconds(value: f64) -> Cycle {
    (value * 64_000_000.0).round() as Cycle
}

// ============================================================================
// Fake board
// ============================================================================

/// A 256-byte register file: the first written byte picks a register and each access moves to the next one.
#[derive(Clone)]
pub struct FakeI2cDevice {
    pub registers: [u8; 256],
    pub pointer: u8,
}

enum I2cSegment {
    Write(u8, Vec<u8>),
    Read(u8, usize),
}

/// Records what the chip sends: SPI bytes answered with their inverse, I2C traffic in the
/// Renode test device's log format, pin announcements, and analog inputs to supply.
#[derive(Default)]
pub struct FakeBoard {
    pub spi_received: HashMap<u8, Vec<u8>>,
    pub i2c_devices: HashMap<u8, FakeI2cDevice>,
    pub i2c_log: Vec<String>,
    segment: Option<I2cSegment>,
    addressed_since_stop: bool,
    pub analog_volts: [Option<f32>; 8],
    pub pin_announcements: Vec<(u32, u32)>,
}

impl FakeBoard {
    pub fn add_i2c_device(&mut self, address: u8) {
        self.i2c_devices.insert(
            address,
            FakeI2cDevice {
                registers: [0; 256],
                pointer: 0,
            },
        );
    }

    pub fn spi_bytes(&self, instance: u8) -> String {
        self.spi_received
            .get(&instance)
            .map(|bytes| {
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }

    pub fn i2c_log(&self) -> String {
        self.i2c_log.join(" | ")
    }

    pub fn last_pins(&self) -> (u32, u32) {
        *self.pin_announcements.last().unwrap_or(&(0, 0))
    }

    /// The level the board sees on a pin: driven pins at their level, others at the PineTime pull-ups.
    pub fn sees_high(&self, pin: u8) -> bool {
        let (levels, _) = self.last_pins();
        levels >> pin & 1 != 0
    }

    fn finish_segment(&mut self) {
        match self.segment.take() {
            Some(I2cSegment::Write(address, bytes)) => {
                let text: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
                self.i2c_log.push(format!("W {}", text.join(" ")));
                if let (Some(device), Some((&first, rest))) =
                    (self.i2c_devices.get_mut(&address), bytes.split_first())
                {
                    device.pointer = first;
                    for &byte in rest {
                        device.registers[device.pointer as usize] = byte;
                        device.pointer = device.pointer.wrapping_add(1);
                    }
                }
            }
            Some(I2cSegment::Read(_, count)) => self.i2c_log.push(format!("R {count}")),
            None => {}
        }
    }
}

impl BoardPorts for FakeBoard {
    fn pin_outputs_changed(&mut self, levels: u32, outputs: u32, _context: &mut dyn DeviceContext) {
        self.pin_announcements.push((levels, outputs));
    }

    fn spi_transfer(&mut self, instance: u8, byte: u8, _context: &mut dyn DeviceContext) -> u8 {
        self.spi_received.entry(instance).or_default().push(byte);
        !byte
    }

    fn i2c_start(
        &mut self,
        _instance: u8,
        address: u8,
        read: bool,
        _context: &mut dyn DeviceContext,
    ) -> bool {
        self.finish_segment();
        if !self.i2c_devices.contains_key(&address) {
            return false;
        }
        self.addressed_since_stop = true;
        self.segment = Some(if read {
            I2cSegment::Read(address, 0)
        } else {
            I2cSegment::Write(address, Vec::new())
        });
        true
    }

    fn i2c_write(&mut self, _instance: u8, byte: u8, _context: &mut dyn DeviceContext) -> bool {
        match &mut self.segment {
            Some(I2cSegment::Write(_, bytes)) => {
                bytes.push(byte);
                true
            }
            _ => false,
        }
    }

    fn i2c_read(&mut self, _instance: u8, _context: &mut dyn DeviceContext) -> u8 {
        let Some(I2cSegment::Read(address, count)) = &mut self.segment else {
            return 0xFF;
        };
        *count += 1;
        let device = self.i2c_devices.get_mut(address).expect("addressed device");
        let value = device.registers[device.pointer as usize];
        device.pointer = device.pointer.wrapping_add(1);
        value
    }

    fn i2c_stop(&mut self, _instance: u8, _context: &mut dyn DeviceContext) {
        self.finish_segment();
        if std::mem::take(&mut self.addressed_since_stop) {
            self.i2c_log.push("F".to_string());
        }
    }

    fn analog_input_volts(&self, channel: u8) -> Option<f32> {
        self.analog_volts[channel as usize]
    }

    fn handle_event(&mut self, _key: EventKey, _context: &mut dyn DeviceContext) {}
}

// ============================================================================
// Harness
// ============================================================================

pub struct Harness {
    pub chip: Nrf52832,
    pub scheduler: Scheduler,
    pub board: FakeBoard,
    pub now: Cycle,
    pub resets: Vec<ResetKind>,
}

impl Default for Harness {
    fn default() -> Harness {
        Harness::new()
    }
}

impl Harness {
    /// An nRF52832 after a power-on reset, with the event trace on.
    pub fn new() -> Harness {
        Harness::new_variant(Variant::Nrf52832)
    }

    /// A chip of `variant` after a power-on reset, with the event trace on.
    pub fn new_variant(variant: Variant) -> Harness {
        let mut harness = Harness {
            chip: Nrf52832::new_variant(variant),
            scheduler: Scheduler::new(),
            board: FakeBoard::default(),
            now: 0,
            resets: Vec::new(),
        };
        harness.with_ports(|chip, ports| chip.reset(ResetKind::PowerOn, ports));
        harness.chip.ram_mut().fill(0);
        harness.chip.take_interrupt_lines();
        harness.chip.set_event_trace(true);
        harness
    }

    pub fn with_ports<R>(&mut self, call: impl FnOnce(&mut Nrf52832, &mut ChipPorts) -> R) -> R {
        let mut ports = ChipPorts {
            now: self.now,
            scheduler: &mut self.scheduler,
            board: &mut self.board,
        };
        call(&mut self.chip, &mut ports)
    }

    pub fn write(&mut self, address: u32, value: u32) {
        self.with_ports(|chip, ports| chip.write(address, value, AccessSize::Word, ports))
            .expect("write");
    }

    pub fn read(&mut self, address: u32) -> u32 {
        self.with_ports(|chip, ports| chip.read(address, AccessSize::Word, ports))
            .expect("read")
    }

    pub fn read_halfword(&mut self, address: u32) -> u32 {
        self.with_ports(|chip, ports| chip.read(address, AccessSize::Halfword, ports))
            .expect("read")
    }

    pub fn write_words(&mut self, address: u32, words: &[u32]) {
        for (index, &word) in words.iter().enumerate() {
            self.write(address + 4 * index as u32, word);
        }
    }

    pub fn drive(&mut self, pin: u8, level: Option<bool>) {
        self.with_ports(|chip, ports| chip.drive_pin(PinChange { pin, level }, ports));
    }

    pub fn set_board_pull(&mut self, pin: u8, pull: Option<bool>) {
        self.with_ports(|chip, ports| chip.set_board_pull(pin, pull, ports));
    }

    pub fn interrupt_line(&self, line: u8) -> bool {
        self.chip.interrupt_lines() >> line & 1 != 0
    }

    pub fn pin_level(&self, pin: u8) -> bool {
        self.chip.pin_input_levels() >> pin & 1 != 0
    }

    /// Runs every event due up to `target`, each at its own cycle, resetting the chip when it asks.
    pub fn advance_to(&mut self, target: Cycle) {
        while let Some((cycle, key)) = self.scheduler.pop_due(target) {
            self.now = self.now.max(cycle);
            if let Owner::Peripheral(_) = key.owner {
                self.with_ports(|chip, ports| chip.handle_event(key, ports));
            }
            if let Some(kind) = self.chip.take_reset_request() {
                self.resets.push(kind);
                self.with_ports(|chip, ports| chip.reset(kind, ports));
            }
        }
        self.now = self.now.max(target);
    }

    pub fn run_for(&mut self, cycles: Cycle) {
        self.advance_to(self.now + cycles);
    }

    pub fn run_for_seconds(&mut self, value: f64) {
        self.run_for(seconds(value));
    }

    pub fn run_for_microseconds(&mut self, value: f64) {
        self.run_for(microseconds(value));
    }

    pub fn next_deadline(&mut self) -> Option<Cycle> {
        self.scheduler.next_deadline()
    }
}

// ============================================================================
// Radio medium
// ============================================================================

/// Collects transmitted frames so a test can hand them to another chip.
#[derive(Clone, Default)]
pub struct Outbox {
    pub frames: Arc<Mutex<Vec<RadioFrame>>>,
}

impl RadioMedium for Outbox {
    fn transmit(&mut self, frame: RadioFrame) {
        self.frames.lock().expect("outbox").push(frame);
    }
}

impl Outbox {
    pub fn take(&self) -> Vec<RadioFrame> {
        std::mem::take(&mut *self.frames.lock().expect("outbox"))
    }
}
