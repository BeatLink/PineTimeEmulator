// SPDX-License-Identifier: GPL-3.0-only
//! What a peripheral may reach while it handles an access or an event: time, the scheduler, the
//! board, memory for EasyDMA, and a queue of what it signals back to the chip.

use std::collections::VecDeque;

use pinetime_core::{BoardPorts, Cycle, EventKey, Owner, PinChange, QueuedContext, Scheduler};

use crate::chip::ResetKind;
use crate::memory::Memory;

// ============================================================================
// Signals
// ============================================================================

/// What peripherals hand back to the chip, which acts on it once the peripheral returns.
#[derive(Default)]
pub(crate) struct Signals {
    /// Addresses of events raised and not yet passed to the PPI.
    pub raised_events: VecDeque<u32>,
    /// Pin changes the board asked for during a call.
    pub pin_changes: Vec<PinChange>,
    /// Peripheral ids whose interrupt line may have changed.
    pub touched: u64,
    pub reset_request: Option<ResetKind>,
    /// Set whenever something was scheduled, so the machine looks at the next deadline again.
    pub schedule_changed: bool,
    pub event_trace: Option<Vec<(Cycle, u32)>>,
}

// ============================================================================
// Context
// ============================================================================

pub(crate) struct Context<'a> {
    pub now: Cycle,
    pub scheduler: &'a mut Scheduler,
    pub board: &'a mut dyn BoardPorts,
    pub memory: &'a mut Memory,
    pub signals: &'a mut Signals,
}

/// The peripheral id of an address on the peripheral bus: bits 12..17.
pub(crate) const fn peripheral_id(address: u32) -> u8 {
    ((address >> 12) & 0x3F) as u8
}

pub(crate) const fn event_key(peripheral: u8, token: u32) -> EventKey {
    EventKey::new(Owner::Peripheral(peripheral), token)
}

impl Context<'_> {
    /// Raises the event at `address`: it goes to the PPI and may change its peripheral's interrupt line.
    pub fn raise_event(&mut self, address: u32) {
        self.signals.raised_events.push_back(address);
        self.signals.touched |= 1u64 << peripheral_id(address);
        if let Some(trace) = &mut self.signals.event_trace {
            trace.push((self.now, address));
        }
    }

    pub fn touch(&mut self, peripheral: u8) {
        self.signals.touched |= 1u64 << peripheral;
    }

    /// Queues `key` for `cycle`, or for now if `cycle` has already passed.
    pub fn schedule(&mut self, key: EventKey, cycle: Cycle) {
        self.scheduler.schedule(key, cycle.max(self.now));
        self.signals.schedule_changed = true;
    }

    pub fn cancel(&mut self, key: EventKey) {
        self.scheduler.cancel(key);
    }

    pub fn request_reset(&mut self, kind: ResetKind) {
        if self.signals.reset_request.is_none() {
            self.signals.reset_request = Some(kind);
        }
    }

    // ========================================================================
    // Board
    // ========================================================================

    fn with_board<R>(&mut self, call: impl FnOnce(&mut dyn BoardPorts, &mut QueuedContext<'_>) -> R) -> R {
        let mut queued = QueuedContext::new(self.now, self.scheduler);
        let result = call(self.board, &mut queued);
        self.signals.pin_changes.append(&mut queued.pin_changes);
        self.signals.schedule_changed = true;
        result
    }

    pub fn spi_transfer(&mut self, instance: u8, byte: u8) -> u8 {
        self.with_board(|board, queued| board.spi_transfer(instance, byte, queued))
    }

    pub fn i2c_start(&mut self, instance: u8, address: u8, read: bool) -> bool {
        self.with_board(|board, queued| board.i2c_start(instance, address, read, queued))
    }

    pub fn i2c_write(&mut self, instance: u8, byte: u8) -> bool {
        self.with_board(|board, queued| board.i2c_write(instance, byte, queued))
    }

    pub fn i2c_read(&mut self, instance: u8) -> u8 {
        self.with_board(|board, queued| board.i2c_read(instance, queued))
    }

    pub fn i2c_stop(&mut self, instance: u8) {
        self.with_board(|board, queued| board.i2c_stop(instance, queued))
    }

    pub fn pin_outputs_changed(&mut self, levels: u32, outputs: u32) {
        self.with_board(|board, queued| board.pin_outputs_changed(levels, outputs, queued))
    }

    pub fn analog_input_volts(&self, channel: u8) -> Option<f32> {
        self.board.analog_input_volts(channel)
    }
}
