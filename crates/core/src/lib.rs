// SPDX-License-Identifier: GPL-3.0-only
//! The pieces every part of the emulated watch shares: emulated time, the event scheduler, the
//! memory bus the CPU talks to, and the ports between the nRF52832 and the parts on the board.

pub mod board;
pub mod bus;
pub mod scheduler;
pub mod time;

pub use board::{BoardPorts, DeviceContext, PinChange, QueuedContext};
pub use bus::{AccessSize, Bus, BusError};
pub use scheduler::{EventKey, Owner, Scheduler};
pub use time::{Cycle, CPU_HERTZ};
