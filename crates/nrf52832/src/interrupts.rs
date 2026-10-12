// SPDX-License-Identifier: GPL-3.0-only
//! The interrupt lines from the peripherals to the CPU's NVIC: line n belongs to the peripheral with id n.

#[derive(Default)]
pub(crate) struct InterruptLines {
    high: u64,
    changed: bool,
}

impl InterruptLines {
    pub fn set(&mut self, line: u8, high: bool) {
        let bit = 1u64 << line;
        let lines = if high { self.high | bit } else { self.high & !bit };
        if lines != self.high {
            self.high = lines;
            self.changed = true;
        }
    }

    pub fn high(&self) -> u64 {
        self.high
    }

    /// The lines held high, if any changed since the last call.
    pub fn take_if_changed(&mut self) -> Option<u64> {
        if !self.changed {
            return None;
        }
        self.changed = false;
        Some(self.high)
    }

    /// Drops every line, and reports the change once even if none was high.
    pub fn clear(&mut self) {
        self.high = 0;
        self.changed = true;
    }
}
