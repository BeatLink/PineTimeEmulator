// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime's Hynitron CST816S touch panel: one finger, gestures, an interrupt line and a reset pin.

use std::collections::VecDeque;

use pinetime_core::time::{cycles_from_microseconds, cycles_from_milliseconds};
use pinetime_core::{Cycle, DeviceContext, EventKey, Owner, PinChange};

use crate::board::{TOUCH_DEVICE, TOUCH_INTERRUPT_PIN};
use crate::i2c::I2cDevice;

// ============================================================================
// Registers and timings
// ============================================================================

const GESTURE_ID: u8 = 0x01;
const FINGER_NUM: u8 = 0x02;
const XPOS_H: u8 = 0x03;
const XPOS_L: u8 = 0x04;
const YPOS_H: u8 = 0x05;
const YPOS_L: u8 = 0x06;
const POWER_MODE: u8 = 0xA5;
const CHIP_ID: u8 = 0xA7;
const PROJ_ID: u8 = 0xA8;
const FW_VERSION: u8 = 0xA9;
const MOTION_MASK: u8 = 0xEC;
const IRQ_PULSE_WIDTH: u8 = 0xED;
const NOR_SCAN_PER: u8 = 0xEE;
const AUTO_SLEEP_TIME: u8 = 0xF9;
const IRQ_CTL: u8 = 0xFA;
const AUTO_RESET: u8 = 0xFB;
const LONG_PRESS_TIME: u8 = 0xFC;
const DIS_AUTO_SLEEP: u8 = 0xFE;

const CHIP_ID_VALUE: u8 = 0xB4;
const PROJECT_ID_VALUE: u8 = 0x00;
const FIRMWARE_VERSION_VALUE: u8 = 0x01;
const DEEP_SLEEP_COMMAND: u8 = 0x03;

const INTERRUPT_ON_TOUCH: u8 = 0x40;
const INTERRUPT_ON_CHANGE: u8 = 0x20;
const INTERRUPT_ON_MOTION: u8 = 0x10;
const MOTION_MASK_DOUBLE_TAP: u8 = 0x01;

const EVENT_SHIFT: u8 = 6;
const EVENT_DOWN: u8 = 0;
const EVENT_UP: u8 = 1;
const EVENT_CONTACT: u8 = 2;
const EVENT_NONE: u8 = 3;

pub const PANEL_SIZE: i32 = 240;
const SLIDE_THRESHOLD_PIXELS: i32 = 40;
const MOVE_TOLERANCE_PIXELS: i32 = 10;

const LONG_PRESS_MILLISECONDS: u64 = 1000;
const LONG_PRESS_RELEASE_DELAY_MILLISECONDS: u64 = 200;
const TAP_HOLD_MILLISECONDS: u64 = 60;
const DOUBLE_TAP_GAP_MILLISECONDS: u64 = 120;
const DOUBLE_TAP_WINDOW_MILLISECONDS: u64 = 300;
const SWIPE_STEPS: i32 = 8;
const SWIPE_STEP_MILLISECONDS: u64 = 20;

const TOKEN_INTERRUPT_PULSE_END: u32 = 0;
const TOKEN_LONG_PRESS_CHECK: u32 = 1;
const TOKEN_PERIODIC_REPORT: u32 = 2;
const TOKEN_SINGLE_TAP_CONFIRM: u32 = 3;
const TOKEN_SCRIPTED_STEP: u32 = 4;

/// The gesture codes the chip reports in GESTURE_ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Gesture {
    None = 0x00,
    SlideDown = 0x01,
    SlideUp = 0x02,
    SlideLeft = 0x03,
    SlideRight = 0x04,
    SingleTap = 0x05,
    DoubleTap = 0x0B,
    LongPress = 0x0C,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

impl SwipeDirection {
    /// Reads "up", "down", "left" or "right", in any case.
    pub fn parse(text: &str) -> Option<SwipeDirection> {
        match text.trim().to_ascii_lowercase().as_str() {
            "up" => Some(SwipeDirection::Up),
            "down" => Some(SwipeDirection::Down),
            "left" => Some(SwipeDirection::Left),
            "right" => Some(SwipeDirection::Right),
            _ => None,
        }
    }

    /// The start and end of a drag across the middle of the panel.
    fn path(self) -> ((i32, i32), (i32, i32)) {
        match self {
            SwipeDirection::Up => ((120, 200), (120, 40)),
            SwipeDirection::Down => ((120, 40), (120, 200)),
            SwipeDirection::Left => ((200, 120), (40, 120)),
            SwipeDirection::Right => ((40, 120), (200, 120)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptedStep {
    Press(i32, i32),
    Drag(i32, i32),
    Lift,
}

fn event_key(token: u32) -> EventKey {
    EventKey::new(Owner::Device(TOUCH_DEVICE), token)
}

// ============================================================================
// Touch panel state
// ============================================================================

pub struct Touch {
    registers: [u8; 256],
    register_pointer: u8,
    expecting_register_address: bool,
    ignoring_this_write: bool,
    held_in_reset: bool,
    deep_sleep: bool,
    last_activity: Cycle,
    ignores_bus_when_idle: bool,

    finger_is_down: bool,
    finger_moved: bool,
    finger_x: i32,
    finger_y: i32,
    start_x: i32,
    start_y: i32,
    touch_start: Cycle,
    gesture: Gesture,
    finger_event: u8,

    tap_pending: bool,
    second_tap_possible: bool,
    last_tap_time: Cycle,

    pointer_x: i32,
    pointer_y: i32,
    pointer_pressed: bool,

    interrupt_pulse_active: bool,
    interrupt_level: Option<bool>,
    scripted_steps: VecDeque<(Cycle, ScriptedStep)>,
}

impl Default for Touch {
    fn default() -> Touch {
        Touch::new()
    }
}

impl Touch {
    pub fn new() -> Touch {
        let mut touch = Touch {
            registers: [0; 256],
            register_pointer: 0,
            expecting_register_address: false,
            ignoring_this_write: false,
            held_in_reset: false,
            deep_sleep: false,
            last_activity: 0,
            ignores_bus_when_idle: false,
            finger_is_down: false,
            finger_moved: false,
            finger_x: 0,
            finger_y: 0,
            start_x: 0,
            start_y: 0,
            touch_start: 0,
            gesture: Gesture::None,
            finger_event: EVENT_NONE,
            tap_pending: false,
            second_tap_possible: false,
            last_tap_time: 0,
            pointer_x: PANEL_SIZE / 2,
            pointer_y: PANEL_SIZE / 2,
            pointer_pressed: false,
            interrupt_pulse_active: false,
            interrupt_level: None,
            scripted_steps: VecDeque::new(),
        };
        touch.restore_register_defaults();
        touch
    }

    /// Starts afresh with the interrupt line idle high; only the reset pin ends a held reset.
    pub fn reset(&mut self, context: &mut dyn DeviceContext) {
        self.restore_register_defaults();
        self.deep_sleep = false;
        self.register_pointer = 0;
        self.forget_touch(context);
        self.pointer_x = PANEL_SIZE / 2;
        self.pointer_y = PANEL_SIZE / 2;
        self.pointer_pressed = false;
        self.last_activity = context.now();
        self.interrupt_level = None;
        self.set_interrupt_line(true, context);
    }

    // ========================================================================
    // State for the emulator and tests
    // ========================================================================

    /// The level the chip drives on its interrupt line, idle high.
    pub fn interrupt_line_high(&self) -> bool {
        self.interrupt_level.unwrap_or(true)
    }

    pub fn finger_is_down(&self) -> bool {
        self.finger_is_down
    }

    pub fn held_in_reset(&self) -> bool {
        self.held_in_reset
    }

    pub fn deep_sleep(&self) -> bool {
        self.deep_sleep
    }

    pub fn gesture(&self) -> Gesture {
        self.gesture
    }

    /// When set, the chip ignores the bus once it has been idle for its auto-sleep time.
    pub fn set_ignores_bus_when_idle(&mut self, ignores: bool) {
        self.ignores_bus_when_idle = ignores;
    }

    // ========================================================================
    // Inputs
    // ========================================================================

    /// Puts a finger down.
    pub fn touch(&mut self, x: i32, y: i32, context: &mut dyn DeviceContext) {
        self.cancel_scripted_steps(context);
        self.finger_down(x, y, context);
    }

    /// Moves the finger that is already down.
    pub fn move_finger(&mut self, x: i32, y: i32, context: &mut dyn DeviceContext) {
        self.cancel_scripted_steps(context);
        self.finger_move(x, y, context);
    }

    /// Lifts the finger.
    pub fn release(&mut self, context: &mut dyn DeviceContext) {
        self.cancel_scripted_steps(context);
        self.pointer_pressed = false;
        self.finger_up(context);
    }

    /// A short touch and lift at one place, reported as a single tap.
    pub fn tap(&mut self, x: i32, y: i32, context: &mut dyn DeviceContext) {
        self.cancel_scripted_steps(context);
        self.finger_down(x, y, context);
        self.after(TAP_HOLD_MILLISECONDS, ScriptedStep::Lift, context);
    }

    /// Two quick taps at one place, a double tap when the motion mask allows it.
    pub fn double_tap(&mut self, x: i32, y: i32, context: &mut dyn DeviceContext) {
        self.cancel_scripted_steps(context);
        self.finger_down(x, y, context);
        self.after(TAP_HOLD_MILLISECONDS, ScriptedStep::Lift, context);
        let second_touch = TAP_HOLD_MILLISECONDS + DOUBLE_TAP_GAP_MILLISECONDS;
        self.after(second_touch, ScriptedStep::Press(x, y), context);
        self.after(second_touch + TAP_HOLD_MILLISECONDS, ScriptedStep::Lift, context);
    }

    /// A finger held still until the panel reports a long press, then lifted.
    pub fn long_press(&mut self, x: i32, y: i32, context: &mut dyn DeviceContext) {
        self.cancel_scripted_steps(context);
        self.finger_down(x, y, context);
        let release = LONG_PRESS_MILLISECONDS + LONG_PRESS_RELEASE_DELAY_MILLISECONDS;
        self.after(release, ScriptedStep::Lift, context);
    }

    /// A finger dragged across the middle of the panel in the direction given.
    pub fn swipe(&mut self, direction: SwipeDirection, context: &mut dyn DeviceContext) {
        let ((start_x, start_y), (end_x, end_y)) = direction.path();
        self.cancel_scripted_steps(context);
        self.finger_down(start_x, start_y, context);
        for step in 1..=SWIPE_STEPS {
            let x = start_x + (end_x - start_x) * step / SWIPE_STEPS;
            let y = start_y + (end_y - start_y) * step / SWIPE_STEPS;
            self.after(
                step as u64 * SWIPE_STEP_MILLISECONDS,
                ScriptedStep::Drag(x, y),
                context,
            );
        }
        let release = (SWIPE_STEPS as u64 + 1) * SWIPE_STEP_MILLISECONDS;
        self.after(release, ScriptedStep::Lift, context);
    }

    /// The window's pointer: moving it while pressed drags the finger.
    pub fn move_to(&mut self, x: i32, y: i32, context: &mut dyn DeviceContext) {
        self.pointer_x = x;
        self.pointer_y = y;
        if self.pointer_pressed {
            self.cancel_scripted_steps(context);
            self.finger_move(x, y, context);
        }
    }

    /// The window's pointer button puts the finger down where the pointer is.
    pub fn press(&mut self, context: &mut dyn DeviceContext) {
        self.cancel_scripted_steps(context);
        self.pointer_pressed = true;
        self.finger_down(self.pointer_x, self.pointer_y, context);
    }

    /// The reset pin, active low: held low the chip is off, and on release it starts afresh.
    pub fn set_reset_pin(&mut self, high: bool, context: &mut dyn DeviceContext) {
        if !high {
            self.held_in_reset = true;
            self.forget_touch(context);
            self.set_interrupt_line(true, context);
        } else if self.held_in_reset {
            self.held_in_reset = false;
            self.restore_register_defaults();
            self.deep_sleep = false;
            self.last_activity = context.now();
        }
    }

    // ========================================================================
    // Events
    // ========================================================================

    pub fn handle_event(&mut self, token: u32, context: &mut dyn DeviceContext) {
        match token {
            TOKEN_INTERRUPT_PULSE_END => {
                self.interrupt_pulse_active = false;
                self.set_interrupt_line(true, context);
            }
            TOKEN_LONG_PRESS_CHECK => self.check_long_press(context),
            TOKEN_PERIODIC_REPORT => self.periodic_report(context),
            TOKEN_SINGLE_TAP_CONFIRM => self.confirm_single_tap(context),
            TOKEN_SCRIPTED_STEP => self.run_scripted_steps(context),
            _ => {}
        }
    }

    /// Runs a step of a scripted gesture later, unless a newer input replaces the gesture first.
    fn after(&mut self, milliseconds: u64, step: ScriptedStep, context: &mut dyn DeviceContext) {
        let due = context.now() + cycles_from_milliseconds(milliseconds);
        self.scripted_steps.push_back((due, step));
        if let Some((first_due, _)) = self.scripted_steps.front() {
            context.schedule(event_key(TOKEN_SCRIPTED_STEP), *first_due);
        }
    }

    fn cancel_scripted_steps(&mut self, context: &mut dyn DeviceContext) {
        self.scripted_steps.clear();
        context.cancel(event_key(TOKEN_SCRIPTED_STEP));
    }

    fn run_scripted_steps(&mut self, context: &mut dyn DeviceContext) {
        let now = context.now();
        while let Some((due, step)) = self.scripted_steps.front().copied() {
            if due > now {
                context.schedule(event_key(TOKEN_SCRIPTED_STEP), due);
                return;
            }
            self.scripted_steps.pop_front();
            match step {
                ScriptedStep::Press(x, y) => self.finger_down(x, y, context),
                ScriptedStep::Drag(x, y) => self.finger_move(x, y, context),
                ScriptedStep::Lift => self.finger_up(context),
            }
        }
    }

    /// The finger went down or up: the checks queued for the previous touch no longer apply.
    fn start_new_touch(&mut self, context: &mut dyn DeviceContext) {
        context.cancel(event_key(TOKEN_LONG_PRESS_CHECK));
        context.cancel(event_key(TOKEN_PERIODIC_REPORT));
        context.cancel(event_key(TOKEN_SINGLE_TAP_CONFIRM));
    }

    fn after_touch(&mut self, milliseconds: u64, token: u32, context: &mut dyn DeviceContext) {
        context.schedule(
            event_key(token),
            context.now() + cycles_from_milliseconds(milliseconds),
        );
    }

    // ========================================================================
    // Finger and gestures
    // ========================================================================

    fn listening(&self) -> bool {
        !self.held_in_reset && !self.deep_sleep
    }

    fn finger_down(&mut self, x: i32, y: i32, context: &mut dyn DeviceContext) {
        if !self.listening() {
            return;
        }
        let now = context.now();
        self.start_new_touch(context);
        self.finger_is_down = true;
        self.finger_moved = false;
        self.finger_x = clamp_to_panel(x);
        self.finger_y = clamp_to_panel(y);
        self.start_x = self.finger_x;
        self.start_y = self.finger_y;
        self.touch_start = now;
        self.last_activity = now;

        let double_tap_window = cycles_from_milliseconds(DOUBLE_TAP_WINDOW_MILLISECONDS);
        self.second_tap_possible = self.tap_pending && now - self.last_tap_time <= double_tap_window;
        self.tap_pending = false;

        self.gesture = Gesture::None;
        self.finger_event = EVENT_DOWN;
        self.update_report();
        self.report(INTERRUPT_ON_TOUCH | INTERRUPT_ON_CHANGE, context);

        self.after_touch(LONG_PRESS_MILLISECONDS, TOKEN_LONG_PRESS_CHECK, context);
        self.after_touch(self.scan_period_milliseconds(), TOKEN_PERIODIC_REPORT, context);
    }

    fn finger_move(&mut self, x: i32, y: i32, context: &mut dyn DeviceContext) {
        if !self.listening() || !self.finger_is_down {
            return;
        }
        self.finger_x = clamp_to_panel(x);
        self.finger_y = clamp_to_panel(y);
        self.finger_event = EVENT_CONTACT;
        self.last_activity = context.now();

        let delta_x = self.finger_x - self.start_x;
        let delta_y = self.finger_y - self.start_y;
        if delta_x.abs() > MOVE_TOLERANCE_PIXELS || delta_y.abs() > MOVE_TOLERANCE_PIXELS {
            self.finger_moved = true;
        }

        let mut new_gesture = Gesture::None;
        if self.gesture == Gesture::None && delta_x.abs().max(delta_y.abs()) >= SLIDE_THRESHOLD_PIXELS {
            new_gesture = if delta_y.abs() >= delta_x.abs() {
                if delta_y > 0 {
                    Gesture::SlideDown
                } else {
                    Gesture::SlideUp
                }
            } else if delta_x > 0 {
                Gesture::SlideRight
            } else {
                Gesture::SlideLeft
            };
            self.gesture = new_gesture;
        }

        self.update_report();
        let motion = if new_gesture != Gesture::None {
            INTERRUPT_ON_MOTION
        } else {
            0
        };
        self.report(INTERRUPT_ON_TOUCH | INTERRUPT_ON_CHANGE | motion, context);
    }

    fn finger_up(&mut self, context: &mut dyn DeviceContext) {
        if !self.listening() || !self.finger_is_down {
            return;
        }
        let now = context.now();
        self.start_new_touch(context);
        self.finger_is_down = false;
        self.finger_event = EVENT_UP;
        self.last_activity = now;

        let mut new_gesture = Gesture::None;
        let short_tap = self.gesture == Gesture::None
            && !self.finger_moved
            && now - self.touch_start < cycles_from_milliseconds(LONG_PRESS_MILLISECONDS);
        if short_tap {
            if self.second_tap_possible {
                new_gesture = Gesture::DoubleTap;
            } else if self.double_taps_enabled() {
                // The chip holds a tap back while it waits to see whether a second one follows.
                self.tap_pending = true;
                self.last_tap_time = now;
                self.after_touch(DOUBLE_TAP_WINDOW_MILLISECONDS, TOKEN_SINGLE_TAP_CONFIRM, context);
            } else {
                new_gesture = Gesture::SingleTap;
            }
        }
        self.second_tap_possible = false;
        if new_gesture != Gesture::None {
            self.gesture = new_gesture;
        }

        self.update_report();
        let motion = if new_gesture != Gesture::None {
            INTERRUPT_ON_MOTION
        } else {
            0
        };
        self.report(INTERRUPT_ON_CHANGE | motion, context);
    }

    fn check_long_press(&mut self, context: &mut dyn DeviceContext) {
        if self.finger_is_down && !self.finger_moved && self.gesture == Gesture::None {
            self.gesture = Gesture::LongPress;
            self.finger_event = EVENT_CONTACT;
            self.update_report();
            self.report(INTERRUPT_ON_MOTION, context);
        }
    }

    fn confirm_single_tap(&mut self, context: &mut dyn DeviceContext) {
        if self.tap_pending {
            self.tap_pending = false;
            self.gesture = Gesture::SingleTap;
            self.update_report();
            self.report(INTERRUPT_ON_MOTION, context);
        }
    }

    fn periodic_report(&mut self, context: &mut dyn DeviceContext) {
        if !self.finger_is_down {
            return;
        }
        if self.finger_event == EVENT_DOWN {
            self.finger_event = EVENT_CONTACT;
            self.update_report();
        }
        self.report(INTERRUPT_ON_TOUCH, context);
        self.after_touch(self.scan_period_milliseconds(), TOKEN_PERIODIC_REPORT, context);
    }

    fn forget_touch(&mut self, context: &mut dyn DeviceContext) {
        self.start_new_touch(context);
        self.cancel_scripted_steps(context);
        self.finger_is_down = false;
        self.tap_pending = false;
        self.second_tap_possible = false;
        self.interrupt_pulse_active = false;
    }

    fn update_report(&mut self) {
        self.registers[GESTURE_ID as usize] = self.gesture as u8;
        self.registers[FINGER_NUM as usize] = self.finger_is_down as u8;
        self.registers[XPOS_H as usize] =
            (self.finger_event << EVENT_SHIFT) | ((self.finger_x >> 8) as u8 & 0x0F);
        self.registers[XPOS_L as usize] = self.finger_x as u8;
        self.registers[YPOS_H as usize] = (self.finger_y >> 8) as u8 & 0x0F;
        self.registers[YPOS_L as usize] = self.finger_y as u8;
    }

    fn double_taps_enabled(&self) -> bool {
        self.registers[MOTION_MASK as usize] & MOTION_MASK_DOUBLE_TAP != 0
    }

    fn scan_period_milliseconds(&self) -> u64 {
        (self.registers[NOR_SCAN_PER as usize] as u64).max(1) * 10
    }

    // ========================================================================
    // Interrupt line
    // ========================================================================

    /// Pulses the interrupt line low when any of `reasons` is enabled in IRQ_CTL.
    fn report(&mut self, reasons: u8, context: &mut dyn DeviceContext) {
        if self.registers[IRQ_CTL as usize] & reasons == 0 || self.interrupt_pulse_active {
            return;
        }
        self.interrupt_pulse_active = true;
        self.set_interrupt_line(false, context);
        let width = (self.registers[IRQ_PULSE_WIDTH as usize] as u64).max(1) * 100;
        let end = context.now() + cycles_from_microseconds(width);
        context.schedule(event_key(TOKEN_INTERRUPT_PULSE_END), end);
    }

    fn set_interrupt_line(&mut self, high: bool, context: &mut dyn DeviceContext) {
        if self.interrupt_level == Some(high) {
            return;
        }
        self.interrupt_level = Some(high);
        context.drive_pin(PinChange {
            pin: TOUCH_INTERRUPT_PIN,
            level: Some(high),
        });
    }

    // ========================================================================
    // Registers
    // ========================================================================

    fn answers_bus(&self, now: Cycle) -> bool {
        if self.held_in_reset || self.deep_sleep {
            return false;
        }
        if self.ignores_bus_when_idle && !self.finger_is_down && self.registers[DIS_AUTO_SLEEP as usize] == 0
        {
            let auto_sleep = cycles_from_milliseconds(self.registers[AUTO_SLEEP_TIME as usize] as u64 * 1000);
            return now - self.last_activity < auto_sleep;
        }
        true
    }

    fn write_register(&mut self, address: u8, value: u8, context: &mut dyn DeviceContext) {
        match address {
            GESTURE_ID | FINGER_NUM | XPOS_H | XPOS_L | YPOS_H | YPOS_L | CHIP_ID | PROJ_ID | FW_VERSION => {}
            POWER_MODE => {
                self.registers[address as usize] = value;
                if value == DEEP_SLEEP_COMMAND {
                    self.deep_sleep = true;
                    self.forget_touch(context);
                    self.set_interrupt_line(true, context);
                }
            }
            _ => self.registers[address as usize] = value,
        }
    }

    fn restore_register_defaults(&mut self) {
        self.registers = [0; 256];
        self.registers[XPOS_H as usize] = EVENT_NONE << EVENT_SHIFT;
        self.registers[CHIP_ID as usize] = CHIP_ID_VALUE;
        self.registers[PROJ_ID as usize] = PROJECT_ID_VALUE;
        self.registers[FW_VERSION as usize] = FIRMWARE_VERSION_VALUE;
        self.registers[MOTION_MASK as usize] = 0x00;
        self.registers[IRQ_PULSE_WIDTH as usize] = 10;
        self.registers[NOR_SCAN_PER as usize] = 1;
        self.registers[AUTO_SLEEP_TIME as usize] = 2;
        self.registers[IRQ_CTL as usize] = INTERRUPT_ON_MOTION;
        self.registers[AUTO_RESET as usize] = 5;
        self.registers[LONG_PRESS_TIME as usize] = 10;
    }
}

impl I2cDevice for Touch {
    fn start(&mut self, read: bool, context: &mut dyn DeviceContext) {
        if !read {
            self.expecting_register_address = true;
            self.ignoring_this_write = !self.answers_bus(context.now());
        }
    }

    fn write(&mut self, byte: u8, context: &mut dyn DeviceContext) {
        if self.ignoring_this_write {
            return;
        }
        if self.expecting_register_address {
            self.expecting_register_address = false;
            self.register_pointer = byte;
            return;
        }
        self.write_register(self.register_pointer, byte, context);
        self.register_pointer = self.register_pointer.wrapping_add(1);
    }

    /// A chip that does not answer leaves the bus idle high.
    fn read(&mut self, context: &mut dyn DeviceContext) -> u8 {
        if !self.answers_bus(context.now()) {
            return 0xFF;
        }
        let value = self.registers[self.register_pointer as usize];
        self.register_pointer = self.register_pointer.wrapping_add(1);
        value
    }
}

fn clamp_to_panel(value: i32) -> i32 {
    value.clamp(0, PANEL_SIZE - 1)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{read_register, read_registers, write_bytes, TestContext};

    fn powered_touch(context: &mut TestContext) -> Touch {
        let mut touch = Touch::new();
        touch.reset(context);
        touch
    }

    fn run_for(touch: &mut Touch, milliseconds: u64, context: &mut TestContext) {
        context.advance_milliseconds(milliseconds, |key, context| {
            assert_eq!(key.owner, Owner::Device(TOUCH_DEVICE));
            touch.handle_event(key.token, context);
        });
    }

    fn report(touch: &mut Touch, context: &mut TestContext) -> Vec<u8> {
        read_registers(touch, GESTURE_ID, 6, context)
    }

    #[test]
    fn reports_its_chip_id() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        assert_eq!(read_register(&mut touch, CHIP_ID, &mut context), 0xB4);
        assert_eq!(
            read_registers(&mut touch, CHIP_ID, 3, &mut context),
            vec![0xB4, 0x00, 0x01]
        );
    }

    #[test]
    fn touch_raises_the_interrupt_and_reports_coordinates() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        write_bytes(&mut touch, &[MOTION_MASK, 0x05], &mut context);
        write_bytes(&mut touch, &[IRQ_CTL, 0x70], &mut context);
        assert!(touch.interrupt_line_high());
        assert_eq!(context.driven_level(TOUCH_INTERRUPT_PIN), Some(true));

        touch.touch(100, 200, &mut context);

        assert!(!touch.interrupt_line_high());
        assert_eq!(context.driven_level(TOUCH_INTERRUPT_PIN), Some(false));
        assert_eq!(
            report(&mut touch, &mut context),
            vec![0x00, 0x01, 0x00, 0x64, 0x00, 0xC8]
        );
        run_for(&mut touch, 2, &mut context);
        assert!(touch.interrupt_line_high());
        assert_eq!(context.driven_level(TOUCH_INTERRUPT_PIN), Some(true));
        touch.move_finger(102, 201, &mut context);
        assert_eq!(
            report(&mut touch, &mut context),
            vec![0x00, 0x01, 0x80, 0x66, 0x00, 0xC9]
        );
    }

    #[test]
    fn the_interrupt_pulse_lasts_the_configured_width() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        write_bytes(&mut touch, &[IRQ_CTL, 0x70], &mut context);
        touch.touch(10, 10, &mut context);

        context.advance_to(cycles_from_microseconds(999), |key, context| {
            touch.handle_event(key.token, context)
        });
        assert!(!touch.interrupt_line_high());
        context.advance_to(cycles_from_microseconds(1000), |key, context| {
            touch.handle_event(key.token, context)
        });
        assert!(touch.interrupt_line_high());
    }

    #[test]
    fn release_ends_the_touch_with_a_tap() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        touch.touch(30, 40, &mut context);
        run_for(&mut touch, 50, &mut context);
        touch.release(&mut context);
        assert_eq!(
            report(&mut touch, &mut context),
            vec![0x05, 0x00, 0x40, 0x1E, 0x00, 0x28]
        );
    }

    #[test]
    fn tap_reports_a_single_tap_after_the_hold() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        touch.tap(20, 20, &mut context);
        assert!(touch.finger_is_down());
        run_for(&mut touch, 100, &mut context);
        assert!(!touch.finger_is_down());
        assert_eq!(touch.gesture(), Gesture::SingleTap);
    }

    #[test]
    fn swipe_reports_the_slide_gesture_while_touching() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        touch.swipe(SwipeDirection::Up, &mut context);
        run_for(&mut touch, 100, &mut context);
        assert_eq!(
            read_registers(&mut touch, GESTURE_ID, 2, &mut context),
            vec![0x02, 0x01]
        );
        run_for(&mut touch, 200, &mut context);
        assert_eq!(
            read_registers(&mut touch, GESTURE_ID, 2, &mut context),
            vec![0x02, 0x00]
        );
    }

    #[test]
    fn swipes_use_the_fingers_direction() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        for (direction, gesture) in [
            ("down", Gesture::SlideDown),
            ("left", Gesture::SlideLeft),
            ("right", Gesture::SlideRight),
        ] {
            touch.swipe(SwipeDirection::parse(direction).unwrap(), &mut context);
            run_for(&mut touch, 300, &mut context);
            assert_eq!(read_register(&mut touch, GESTURE_ID, &mut context), gesture as u8);
        }
        assert_eq!(SwipeDirection::parse(" UP "), Some(SwipeDirection::Up));
        assert_eq!(SwipeDirection::parse("sideways"), None);
    }

    #[test]
    fn mouse_drag_becomes_a_slide() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        touch.move_to(120, 60, &mut context);
        touch.press(&mut context);
        run_for(&mut touch, 20, &mut context);
        touch.move_to(120, 90, &mut context);
        run_for(&mut touch, 20, &mut context);
        assert_eq!(read_register(&mut touch, GESTURE_ID, &mut context), 0x00);
        touch.move_to(120, 160, &mut context);
        assert_eq!(
            report(&mut touch, &mut context),
            vec![0x01, 0x01, 0x80, 0x78, 0x00, 0xA0]
        );
        run_for(&mut touch, 20, &mut context);
        touch.release(&mut context);
        assert_eq!(
            report(&mut touch, &mut context),
            vec![0x01, 0x00, 0x40, 0x78, 0x00, 0xA0]
        );
    }

    #[test]
    fn moving_the_pointer_without_pressing_leaves_the_finger_up() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        touch.move_to(50, 50, &mut context);
        assert!(!touch.finger_is_down());
        assert_eq!(read_register(&mut touch, FINGER_NUM, &mut context), 0);
    }

    #[test]
    fn long_press_and_double_tap() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        touch.long_press(50, 60, &mut context);
        run_for(&mut touch, 1100, &mut context);
        assert_eq!(
            report(&mut touch, &mut context),
            vec![0x0C, 0x01, 0x80, 0x32, 0x00, 0x3C]
        );
        run_for(&mut touch, 200, &mut context);
        write_bytes(&mut touch, &[MOTION_MASK, 0x01], &mut context);
        touch.double_tap(70, 80, &mut context);
        run_for(&mut touch, 100, &mut context);
        assert_eq!(read_register(&mut touch, GESTURE_ID, &mut context), 0x00);
        run_for(&mut touch, 200, &mut context);
        assert_eq!(read_register(&mut touch, GESTURE_ID, &mut context), 0x0B);
    }

    #[test]
    fn a_held_back_tap_is_confirmed_when_no_second_tap_follows() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        write_bytes(&mut touch, &[MOTION_MASK, 0x01], &mut context);
        touch.tap(10, 10, &mut context);
        run_for(&mut touch, 200, &mut context);
        assert_eq!(touch.gesture(), Gesture::None);
        run_for(&mut touch, 200, &mut context);
        assert_eq!(touch.gesture(), Gesture::SingleTap);
    }

    #[test]
    fn a_new_input_cancels_the_rest_of_a_scripted_gesture() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        touch.swipe(SwipeDirection::Up, &mut context);
        touch.touch(5, 5, &mut context);
        run_for(&mut touch, 300, &mut context);
        assert!(touch.finger_is_down());
        assert_eq!(read_registers(&mut touch, XPOS_L, 3, &mut context), vec![5, 0, 5]);
    }

    #[test]
    fn sleeps_until_reset() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        write_bytes(&mut touch, &[POWER_MODE, 0x03], &mut context);
        assert_eq!(read_register(&mut touch, CHIP_ID, &mut context), 0xFF);
        touch.touch(10, 10, &mut context);
        assert!(touch.interrupt_line_high());
        touch.set_reset_pin(false, &mut context);
        assert!(touch.held_in_reset());
        touch.set_reset_pin(true, &mut context);
        assert_eq!(read_register(&mut touch, CHIP_ID, &mut context), 0xB4);
    }

    #[test]
    fn read_only_registers_ignore_writes() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        write_bytes(&mut touch, &[CHIP_ID, 0x00, 0x55], &mut context);
        assert_eq!(
            read_registers(&mut touch, CHIP_ID, 2, &mut context),
            vec![0xB4, 0x00]
        );
    }

    #[test]
    fn ignores_the_bus_once_idle_past_the_auto_sleep_time_when_asked() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        touch.set_ignores_bus_when_idle(true);
        assert_eq!(read_register(&mut touch, CHIP_ID, &mut context), 0xB4);
        run_for(&mut touch, 2_000, &mut context);
        assert_eq!(read_register(&mut touch, CHIP_ID, &mut context), 0xFF);

        touch.touch(1, 1, &mut context);
        assert_eq!(read_register(&mut touch, CHIP_ID, &mut context), 0xB4);
    }

    #[test]
    fn periodic_reports_pulse_the_line_while_the_finger_is_down() {
        let mut context = TestContext::new();
        let mut touch = powered_touch(&mut context);
        write_bytes(&mut touch, &[IRQ_CTL, INTERRUPT_ON_TOUCH], &mut context);
        touch.touch(10, 10, &mut context);
        let lows_after_touch = context
            .pin_changes
            .iter()
            .filter(|change| change.level == Some(false))
            .count();

        run_for(&mut touch, 35, &mut context);

        let lows = context
            .pin_changes
            .iter()
            .filter(|change| change.level == Some(false))
            .count();
        assert_eq!(lows - lows_after_touch, 3);
        assert_eq!(
            read_register(&mut touch, XPOS_H, &mut context) >> EVENT_SHIFT,
            EVENT_CONTACT
        );
    }
}
