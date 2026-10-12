// SPDX-License-Identifier: GPL-3.0-only
//! The byte-by-byte I2C target side every sensor on the PineTime's bus implements.

use pinetime_core::DeviceContext;

/// A part on the I2C bus, where the first byte written after a start selects the register to use.
pub trait I2cDevice {
    fn start(&mut self, read: bool, context: &mut dyn DeviceContext);

    fn write(&mut self, byte: u8, context: &mut dyn DeviceContext);

    fn read(&mut self, context: &mut dyn DeviceContext) -> u8;

    fn stop(&mut self, _context: &mut dyn DeviceContext) {}
}
