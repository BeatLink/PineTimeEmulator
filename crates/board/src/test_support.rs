// SPDX-License-Identifier: GPL-3.0-only
//! A `DeviceContext` for tests: it keeps its own scheduler and clock and records every pin change.

use pinetime_core::time::cycles_from_milliseconds;
use pinetime_core::{Cycle, DeviceContext, EventKey, PinChange, Scheduler};

use crate::i2c::I2cDevice;

// ============================================================================
// Test context
// ============================================================================

#[derive(Default)]
pub struct TestContext {
    pub now: Cycle,
    pub scheduler: Scheduler,
    pub pin_changes: Vec<PinChange>,
    pub driven_levels: [Option<bool>; 32],
}

impl TestContext {
    pub fn new() -> TestContext {
        TestContext::default()
    }

    /// Runs every event due up to `until`, handing each to `handler` at its own cycle.
    pub fn advance_to(&mut self, until: Cycle, mut handler: impl FnMut(EventKey, &mut TestContext)) {
        while let Some((cycle, key)) = self.scheduler.pop_due(until) {
            self.now = self.now.max(cycle);
            handler(key, self);
        }
        self.now = self.now.max(until);
    }

    pub fn advance_milliseconds(
        &mut self,
        milliseconds: u64,
        handler: impl FnMut(EventKey, &mut TestContext),
    ) {
        let until = self.now + cycles_from_milliseconds(milliseconds);
        self.advance_to(until, handler);
    }

    pub fn driven_level(&self, pin: u8) -> Option<bool> {
        self.driven_levels[pin as usize]
    }
}

impl DeviceContext for TestContext {
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
        self.driven_levels[change.pin as usize] = change.level;
        self.pin_changes.push(change);
    }
}

// ============================================================================
// I2C helpers
// ============================================================================

/// Sends one write transaction: a register address followed by the values written from it.
pub fn write_bytes(device: &mut dyn I2cDevice, bytes: &[u8], context: &mut TestContext) {
    device.start(false, context);
    for byte in bytes {
        device.write(*byte, context);
    }
    device.stop(context);
}

/// Selects `register` and reads `count` bytes from it, as the Robot tests' Read Registers did.
pub fn read_registers(
    device: &mut dyn I2cDevice,
    register: u8,
    count: usize,
    context: &mut TestContext,
) -> Vec<u8> {
    write_bytes(device, &[register], context);
    device.start(true, context);
    let bytes = (0..count).map(|_| device.read(context)).collect();
    device.stop(context);
    bytes
}

pub fn read_register(device: &mut dyn I2cDevice, register: u8, context: &mut TestContext) -> u8 {
    read_registers(device, register, 1, context)[0]
}
