// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime's battery on AIN7 through a /2 divider, and its charger's power and charging lines.

use pinetime_core::{DeviceContext, PinChange};

use crate::board::{CHARGING_PIN, POWER_PRESENT_PIN};

pub const BATTERY_ANALOG_INPUT: u8 = 7;
const DIVIDER_RATIO: f64 = 0.5;
const EMPTY_VOLTS: f64 = 3.0;
const FULL_VOLTS: f64 = 4.2;
const DEFAULT_VOLTS: f64 = 3.9;

// ============================================================================
// Battery state
// ============================================================================

pub struct Battery {
    volts: f64,
    charger_connected: bool,
}

impl Default for Battery {
    fn default() -> Battery {
        Battery::new()
    }
}

impl Battery {
    pub fn new() -> Battery {
        Battery {
            volts: DEFAULT_VOLTS,
            charger_connected: false,
        }
    }

    pub fn volts(&self) -> f64 {
        self.volts
    }

    pub fn set_volts(&mut self, volts: f64, context: &mut dyn DeviceContext) {
        self.volts = volts.clamp(0.0, FULL_VOLTS);
        self.apply(context);
    }

    /// The charge as a share of the usable range, from 3.0 V (empty) to 4.2 V (full).
    pub fn percent(&self) -> i32 {
        let share = ((self.volts - EMPTY_VOLTS) / (FULL_VOLTS - EMPTY_VOLTS)).clamp(0.0, 1.0);
        (share * 100.0).round_ties_even() as i32
    }

    pub fn set_percent(&mut self, percent: i32, context: &mut dyn DeviceContext) {
        let share = percent.clamp(0, 100) as f64 / 100.0;
        self.set_volts(EMPTY_VOLTS + (FULL_VOLTS - EMPTY_VOLTS) * share, context);
    }

    // ========================================================================
    // Charger
    // ========================================================================

    pub fn charger_connected(&self) -> bool {
        self.charger_connected
    }

    pub fn set_charger_connected(&mut self, connected: bool, context: &mut dyn DeviceContext) {
        self.charger_connected = connected;
        self.apply(context);
    }

    pub fn connect_charger(&mut self, context: &mut dyn DeviceContext) {
        self.set_charger_connected(true, context);
    }

    pub fn disconnect_charger(&mut self, context: &mut dyn DeviceContext) {
        self.set_charger_connected(false, context);
    }

    /// While on the charger the cell is reported as charging until it is full.
    pub fn charging(&self) -> bool {
        self.charger_connected && self.volts < FULL_VOLTS
    }

    /// The voltage the divider puts on AIN7.
    pub fn analog_input_volts(&self) -> f32 {
        (self.volts * DIVIDER_RATIO) as f32
    }

    /// Both status lines are open drain from the charger chip, so they are released, not driven high.
    pub fn apply(&mut self, context: &mut dyn DeviceContext) {
        drive_low_or_release(POWER_PRESENT_PIN, self.charger_connected, context);
        drive_low_or_release(CHARGING_PIN, self.charging(), context);
    }
}

fn drive_low_or_release(pin: u8, low: bool, context: &mut dyn DeviceContext) {
    let level = if low { Some(false) } else { None };
    context.drive_pin(PinChange { pin, level });
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestContext;

    #[test]
    fn starts_at_3_9_volts_off_the_charger() {
        let battery = Battery::new();
        assert_eq!(battery.volts(), 3.9);
        assert_eq!(battery.percent(), 75);
        assert!((battery.analog_input_volts() - 1.95).abs() < 1e-6);
        assert!(!battery.charging());
    }

    #[test]
    fn percent_sets_the_voltage_through_the_usable_range() {
        let mut context = TestContext::new();
        let mut battery = Battery::new();
        battery.set_percent(50, &mut context);
        assert!((battery.volts() - 3.6).abs() < 1e-9);
        assert_eq!(battery.percent(), 50);
        assert!((battery.analog_input_volts() - 1.8).abs() < 1e-6);

        battery.set_percent(150, &mut context);
        assert_eq!(battery.volts(), 4.2);
        battery.set_volts(2.0, &mut context);
        assert_eq!(battery.percent(), 0);
    }

    #[test]
    fn the_charger_pulls_its_status_lines_low() {
        let mut context = TestContext::new();
        let mut battery = Battery::new();
        battery.apply(&mut context);
        assert_eq!(context.driven_level(POWER_PRESENT_PIN), None);
        assert_eq!(context.driven_level(CHARGING_PIN), None);

        battery.connect_charger(&mut context);
        assert!(battery.charging());
        assert_eq!(context.driven_level(POWER_PRESENT_PIN), Some(false));
        assert_eq!(context.driven_level(CHARGING_PIN), Some(false));

        battery.set_percent(100, &mut context);
        assert!(!battery.charging());
        assert_eq!(context.driven_level(POWER_PRESENT_PIN), Some(false));
        assert_eq!(context.driven_level(CHARGING_PIN), None);

        battery.disconnect_charger(&mut context);
        assert_eq!(context.driven_level(POWER_PRESENT_PIN), None);
    }
}
