// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime's vibration motor, which runs while P0.16 is low.

#[derive(Default)]
pub struct Motor {
    running: bool,
    starts: u32,
}

impl Motor {
    pub fn new() -> Motor {
        Motor::default()
    }

    pub fn set_pin_level(&mut self, high: bool) {
        let running = !high;
        if running && !self.running {
            self.starts = self.starts.wrapping_add(1);
        }
        self.running = running;
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// Counts each time the motor starts, so a buzz shorter than the window's refresh is still seen.
    pub fn starts(&self) -> u32 {
        self.starts
    }

    pub fn reset(&mut self) {
        self.running = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_while_the_pin_is_low_and_counts_starts() {
        let mut motor = Motor::new();
        assert!(!motor.running());
        motor.set_pin_level(false);
        assert!(motor.running());
        motor.set_pin_level(false);
        assert_eq!(motor.starts(), 1);
        motor.set_pin_level(true);
        assert!(!motor.running());
        motor.set_pin_level(false);
        assert_eq!(motor.starts(), 2);
    }
}
