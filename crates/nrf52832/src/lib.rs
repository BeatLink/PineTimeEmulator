// SPDX-License-Identifier: GPL-3.0-only
//! The Nordic nRF52832: flash, RAM, FICR, UICR and every peripheral the PineTime firmware uses,
//! with an nRF52840 variant for the Bluetooth bridge's controller.
//!
//! The CPU reaches the chip through `Nrf52832::read`, `write` and `fetch`; time passes through
//! the shared `Scheduler`, and the parts on the board are reached only through `BoardPorts`.

mod aes;
mod chip;
mod context;
mod interrupts;
mod memory;
mod peripheral_bus;
mod peripherals;
mod ppi;
mod variant;

pub use chip::{ChipPorts, Nrf52832, ResetKind, SerialPort, WatchAccess, PINETIME_BOARD_PULL_UPS};
pub use memory::{
    CODE_RAM_START, ERRATA_START, FICR_START, FLASH_PAGE_SIZE, FLASH_SIZE, RAM_SIZE, RAM_START, UICR_START,
};
pub use peripherals::radio::{RadioFrame, RadioMedium, SenderTiming};
pub use variant::Variant;
