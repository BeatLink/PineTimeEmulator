// SPDX-License-Identifier: GPL-3.0-only
//! The parts on the PineTime board and the wiring that joins them to the nRF52832.

pub mod accelerometer;
pub mod backlight;
pub mod battery;
pub mod board;
pub mod external_flash;
pub mod heart_rate;
pub mod i2c;
pub mod motor;
pub mod png;
pub mod side_button;
pub mod st7789;
pub mod touch;

#[cfg(test)]
mod test_support;

pub use accelerometer::{Accelerometer, AccelerometerModel};
pub use backlight::Backlight;
pub use battery::Battery;
pub use board::PineTimeBoard;
pub use external_flash::ExternalFlash;
pub use heart_rate::HeartRateSensor;
pub use i2c::I2cDevice;
pub use motor::Motor;
pub use side_button::SideButton;
pub use st7789::St7789;
pub use touch::{SwipeDirection, Touch};
