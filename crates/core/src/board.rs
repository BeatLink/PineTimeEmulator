// SPDX-License-Identifier: GPL-3.0-only
//! The ports between the nRF52832 and the parts on the board: pins, the SPI and I2C buses, and
//! the analog inputs.
//!
//! The chip calls `BoardPorts`; the board answers through a `DeviceContext`, which only queues
//! what the parts want done (pin changes, alarms), so neither side ever holds the other.

use crate::scheduler::{EventKey, Scheduler};
use crate::time::Cycle;

/// A pin a part starts or stops driving; `None` lets the pin float back to its pull.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinChange {
    pub pin: u8,
    pub level: Option<bool>,
}

/// What a part on the board may do while it handles a call from the chip or one of its events.
pub trait DeviceContext {
    fn now(&self) -> Cycle;

    /// Queues `key` to come due at `cycle`, moving it if it is already queued.
    fn schedule(&mut self, key: EventKey, cycle: Cycle);

    fn cancel(&mut self, key: EventKey);

    /// Drives one of the chip's pins from outside, as an interrupt line or a button does.
    fn drive_pin(&mut self, change: PinChange);
}

/// Everything on the board the chip reaches through its pins and buses.
pub trait BoardPorts {
    /// The chip's pin outputs changed: `levels` holds the output level of all 32 pins and
    /// `outputs` marks the pins configured as outputs.
    fn pin_outputs_changed(&mut self, levels: u32, outputs: u32, context: &mut dyn DeviceContext);

    /// Shifts one byte out on SPI master `instance` and returns the byte shifted in, which is
    /// 0xFF when no part is selected.
    fn spi_transfer(&mut self, instance: u8, byte: u8, context: &mut dyn DeviceContext) -> u8;

    /// Sends an I2C start and address on `instance`; returns whether a part acknowledged.
    fn i2c_start(&mut self, instance: u8, address: u8, read: bool, context: &mut dyn DeviceContext) -> bool;

    /// Writes one byte to the addressed part; returns whether it acknowledged.
    fn i2c_write(&mut self, instance: u8, byte: u8, context: &mut dyn DeviceContext) -> bool;

    fn i2c_read(&mut self, instance: u8, context: &mut dyn DeviceContext) -> u8;

    fn i2c_stop(&mut self, instance: u8, context: &mut dyn DeviceContext);

    /// The voltage on analog input AIN`channel`, or `None` when nothing is wired there.
    fn analog_input_volts(&self, channel: u8) -> Option<f32>;

    /// Handles an event the board queued for one of its parts.
    fn handle_event(&mut self, key: EventKey, context: &mut dyn DeviceContext);
}

// ============================================================================
// Queued context
// ============================================================================

/// The `DeviceContext` the chip hands the board: alarms go straight to the scheduler and pin
/// changes wait in `pin_changes` until the chip applies them.
pub struct QueuedContext<'a> {
    pub now: Cycle,
    pub scheduler: &'a mut Scheduler,
    pub pin_changes: Vec<PinChange>,
}

impl<'a> QueuedContext<'a> {
    pub fn new(now: Cycle, scheduler: &'a mut Scheduler) -> QueuedContext<'a> {
        QueuedContext {
            now,
            scheduler,
            pin_changes: Vec::new(),
        }
    }
}

impl DeviceContext for QueuedContext<'_> {
    fn now(&self) -> Cycle {
        self.now
    }

    fn schedule(&mut self, key: EventKey, cycle: Cycle) {
        self.scheduler.schedule(key, cycle);
    }

    fn cancel(&mut self, key: EventKey) {
        self.scheduler.cancel(key);
    }

    fn drive_pin(&mut self, change: PinChange) {
        self.pin_changes.push(change);
    }
}
