// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime's side button, which joins the P0.13 sense pin to the P0.15 enable line while pressed.

use pinetime_core::time::cycles_from_milliseconds;
use pinetime_core::{DeviceContext, EventKey, Owner, PinChange};

use crate::board::{SIDE_BUTTON_DEVICE, SIDE_BUTTON_SENSE_PIN};

const DEFAULT_CLICK_MILLISECONDS: u64 = 100;
const TOKEN_RELEASE: u32 = 0;

fn release_key() -> EventKey {
    EventKey::new(Owner::Device(SIDE_BUTTON_DEVICE), TOKEN_RELEASE)
}

// ============================================================================
// Button state
// ============================================================================

#[derive(Default)]
pub struct SideButton {
    pressed: bool,
    powered: bool,
}

impl SideButton {
    pub fn new() -> SideButton {
        SideButton::default()
    }

    pub fn pressed(&self) -> bool {
        self.pressed
    }

    /// Whether the chip drives the enable line high.
    pub fn powered(&self) -> bool {
        self.powered
    }

    pub fn reset(&mut self, context: &mut dyn DeviceContext) {
        self.powered = false;
        self.update(context);
    }

    /// The enable line on P0.15 changed level.
    pub fn set_enable_pin(&mut self, high: bool, context: &mut dyn DeviceContext) {
        self.powered = high;
        self.update(context);
    }

    // ========================================================================
    // Pressing
    // ========================================================================

    pub fn press(&mut self, context: &mut dyn DeviceContext) {
        context.cancel(release_key());
        self.pressed = true;
        self.update(context);
    }

    pub fn release(&mut self, context: &mut dyn DeviceContext) {
        context.cancel(release_key());
        self.pressed = false;
        self.update(context);
    }

    /// Presses now and lets go after `milliseconds` of emulated time.
    pub fn hold(&mut self, milliseconds: u64, context: &mut dyn DeviceContext) {
        self.press(context);
        context.schedule(
            release_key(),
            context.now() + cycles_from_milliseconds(milliseconds),
        );
    }

    pub fn press_and_release(&mut self, context: &mut dyn DeviceContext) {
        self.hold(DEFAULT_CLICK_MILLISECONDS, context);
    }

    pub fn handle_event(&mut self, token: u32, context: &mut dyn DeviceContext) {
        if token == TOKEN_RELEASE {
            self.pressed = false;
            self.update(context);
        }
    }

    /// A closed switch carries the enable line's level; an open one leaves the pin to its pulls.
    fn update(&mut self, context: &mut dyn DeviceContext) {
        let level = if self.pressed { Some(self.powered) } else { None };
        context.drive_pin(PinChange {
            pin: SIDE_BUTTON_SENSE_PIN,
            level,
        });
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestContext;

    #[test]
    fn reads_pressed_only_while_the_enable_line_is_high() {
        let mut context = TestContext::new();
        let mut button = SideButton::new();
        button.reset(&mut context);
        assert_eq!(context.driven_level(SIDE_BUTTON_SENSE_PIN), None);

        button.press(&mut context);
        assert!(button.pressed());
        assert_eq!(context.driven_level(SIDE_BUTTON_SENSE_PIN), Some(false));

        button.set_enable_pin(true, &mut context);
        assert_eq!(context.driven_level(SIDE_BUTTON_SENSE_PIN), Some(true));

        button.release(&mut context);
        assert!(!button.pressed());
        assert_eq!(context.driven_level(SIDE_BUTTON_SENSE_PIN), None);
    }

    #[test]
    fn press_and_release_lets_go_after_a_tenth_of_a_second() {
        let mut context = TestContext::new();
        let mut button = SideButton::new();
        button.set_enable_pin(true, &mut context);
        button.press_and_release(&mut context);
        context.advance_milliseconds(99, |key, context| button.handle_event(key.token, context));
        assert!(button.pressed());
        context.advance_milliseconds(1, |key, context| button.handle_event(key.token, context));
        assert!(!button.pressed());
        assert_eq!(context.driven_level(SIDE_BUTTON_SENSE_PIN), None);
    }

    #[test]
    fn a_new_press_cancels_a_pending_release() {
        let mut context = TestContext::new();
        let mut button = SideButton::new();
        button.hold(10, &mut context);
        button.press(&mut context);
        context.advance_milliseconds(50, |key, context| button.handle_event(key.token, context));
        assert!(button.pressed());
    }
}
