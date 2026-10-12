// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime board: every part wired to the nRF52832's pins and buses as the schematic has them.

use pinetime_core::{BoardPorts, DeviceContext, EventKey, Owner};

use crate::accelerometer::{Accelerometer, AccelerometerModel};
use crate::backlight::BacklightLine;
use crate::battery::{Battery, BATTERY_ANALOG_INPUT};
use crate::external_flash::ExternalFlash;
use crate::heart_rate::HeartRateSensor;
use crate::i2c::I2cDevice;
use crate::motor::Motor;
use crate::side_button::SideButton;
use crate::st7789::St7789;
use crate::touch::Touch;

// ============================================================================
// Wiring
// ============================================================================

pub const FLASH_CHIP_SELECT_PIN: u8 = 5;
pub const ACCELEROMETER_INTERRUPT_PIN: u8 = 8;
pub const TOUCH_RESET_PIN: u8 = 10;
pub const CHARGING_PIN: u8 = 12;
pub const SIDE_BUTTON_SENSE_PIN: u8 = 13;
pub const BACKLIGHT_LOW_PIN: u8 = 14;
pub const SIDE_BUTTON_ENABLE_PIN: u8 = 15;
pub const MOTOR_PIN: u8 = 16;
pub const DISPLAY_DATA_COMMAND_PIN: u8 = 18;
pub const POWER_PRESENT_PIN: u8 = 19;
pub const BACKLIGHT_MID_PIN: u8 = 22;
pub const BACKLIGHT_HIGH_PIN: u8 = 23;
pub const DISPLAY_CHIP_SELECT_PIN: u8 = 25;
pub const DISPLAY_RESET_PIN: u8 = 26;
pub const TOUCH_INTERRUPT_PIN: u8 = 28;

/// Board pull-ups hold the chip selects (P0.05, P0.25) and the charger lines (P0.12, P0.19) high.
static TRACING_SPI: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Turns printing every SPI byte and the part it reached on standard error on or off.
pub fn trace_spi(on: bool) {
    TRACING_SPI.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub const BOARD_PULL_UPS: u32 = 0x0208_1020;

/// The chip outputs some part listens to.
const WATCHED_PINS: u32 = (1 << FLASH_CHIP_SELECT_PIN)
    | (1 << TOUCH_RESET_PIN)
    | (1 << BACKLIGHT_LOW_PIN)
    | (1 << SIDE_BUTTON_ENABLE_PIN)
    | (1 << MOTOR_PIN)
    | (1 << DISPLAY_DATA_COMMAND_PIN)
    | (1 << BACKLIGHT_MID_PIN)
    | (1 << BACKLIGHT_HIGH_PIN)
    | (1 << DISPLAY_CHIP_SELECT_PIN)
    | (1 << DISPLAY_RESET_PIN);

/// The serial block instance that carries the SPI bus, and the one that carries the I2C bus.
pub const SPI_INSTANCE: u8 = 0;
pub const I2C_INSTANCE: u8 = 1;

pub const TOUCH_ADDRESS: u8 = 0x15;
pub const ACCELEROMETER_ADDRESS: u8 = 0x18;
pub const HEART_RATE_ADDRESS: u8 = 0x44;

/// The numbers that name each part as the owner of its scheduled events.
pub const DISPLAY_DEVICE: u8 = 0;
pub const EXTERNAL_FLASH_DEVICE: u8 = 1;
pub const TOUCH_DEVICE: u8 = 2;
pub const ACCELEROMETER_DEVICE: u8 = 3;
pub const HEART_RATE_DEVICE: u8 = 4;
pub const SIDE_BUTTON_DEVICE: u8 = 5;
pub const BATTERY_DEVICE: u8 = 6;
pub const MOTOR_DEVICE: u8 = 7;

const IDLE_BUS: u8 = 0xFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpiTarget {
    Nothing,
    Display,
    ExternalFlash,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum I2cTarget {
    Touch,
    Accelerometer,
    HeartRate,
}

fn is_high(levels: u32, pin: u8) -> bool {
    levels & (1 << pin) != 0
}

// ============================================================================
// Board
// ============================================================================

pub struct PineTimeBoard {
    display: St7789,
    external_flash: ExternalFlash,
    touch: Touch,
    accelerometer: Accelerometer,
    heart_rate: HeartRateSensor,
    side_button: SideButton,
    battery: Battery,
    motor: Motor,
    pin_levels: u32,
    announce_every_pin: bool,
    spi_target: SpiTarget,
    i2c_target: Option<I2cTarget>,
}

impl PineTimeBoard {
    /// A board whose parts are in their power-on state; call `power_on` before running.
    pub fn new(accelerometer_model: AccelerometerModel) -> PineTimeBoard {
        let mut board = PineTimeBoard {
            display: St7789::new(),
            external_flash: ExternalFlash::new(),
            touch: Touch::new(),
            accelerometer: Accelerometer::new(accelerometer_model),
            heart_rate: HeartRateSensor::new(),
            side_button: SideButton::new(),
            battery: Battery::new(),
            motor: Motor::new(),
            pin_levels: BOARD_PULL_UPS,
            announce_every_pin: false,
            spi_target: SpiTarget::Nothing,
            i2c_target: None,
        };
        board.match_backlight_to_undriven_pins();
        board
    }

    /// The backlight lines start low like every pin without a pull-up, which is full brightness.
    fn match_backlight_to_undriven_pins(&mut self) {
        for line in [BacklightLine::Low, BacklightLine::Mid, BacklightLine::High] {
            self.display.set_backlight_line(line, false);
        }
    }

    /// The pins the board holds high when the chip does not drive them.
    pub fn board_pull_ups() -> u32 {
        BOARD_PULL_UPS
    }

    /// Drives the board's idle input levels: button released, charger state, interrupt lines idle.
    pub fn power_on(&mut self, context: &mut dyn DeviceContext) {
        self.side_button.reset(context);
        self.battery.apply(context);
        self.touch.reset(context);
        self.accelerometer.reset(context);
    }

    /// Resets every part as a machine reset does; the next pin update reaches every part again.
    pub fn reset(&mut self, context: &mut dyn DeviceContext) {
        self.display.reset();
        self.external_flash.reset();
        self.heart_rate.reset(context.now());
        self.motor.reset();
        self.i2c_target = None;
        self.announce_every_pin = true;
        self.power_on(context);
    }

    // ========================================================================
    // Parts
    // ========================================================================

    pub fn display(&self) -> &St7789 {
        &self.display
    }

    pub fn display_mut(&mut self) -> &mut St7789 {
        &mut self.display
    }

    pub fn external_flash(&self) -> &ExternalFlash {
        &self.external_flash
    }

    pub fn external_flash_mut(&mut self) -> &mut ExternalFlash {
        &mut self.external_flash
    }

    pub fn touch(&self) -> &Touch {
        &self.touch
    }

    pub fn touch_mut(&mut self) -> &mut Touch {
        &mut self.touch
    }

    pub fn side_button(&self) -> &SideButton {
        &self.side_button
    }

    pub fn side_button_mut(&mut self) -> &mut SideButton {
        &mut self.side_button
    }

    pub fn battery(&self) -> &Battery {
        &self.battery
    }

    pub fn battery_mut(&mut self) -> &mut Battery {
        &mut self.battery
    }

    pub fn accelerometer(&self) -> &Accelerometer {
        &self.accelerometer
    }

    pub fn accelerometer_mut(&mut self) -> &mut Accelerometer {
        &mut self.accelerometer
    }

    pub fn heart_rate(&self) -> &HeartRateSensor {
        &self.heart_rate
    }

    pub fn heart_rate_mut(&mut self) -> &mut HeartRateSensor {
        &mut self.heart_rate
    }

    pub fn motor_running(&self) -> bool {
        self.motor.running()
    }

    pub fn motor_starts(&self) -> u32 {
        self.motor.starts()
    }

    /// 0 is off, 1 low, 2 mid, 3 full.
    pub fn backlight_level(&self) -> u8 {
        self.display.backlight_level()
    }

    /// The level of each pin as the parts last saw it, one bit per pin.
    pub fn pin_levels(&self) -> u32 {
        self.pin_levels
    }

    // ========================================================================
    // Pins
    // ========================================================================

    fn pin_changed(&mut self, pin: u8, high: bool, context: &mut dyn DeviceContext) {
        match pin {
            DISPLAY_DATA_COMMAND_PIN => self.display.set_data_command_pin(high),
            DISPLAY_RESET_PIN => self.display.set_reset_pin(high),
            BACKLIGHT_LOW_PIN => self.display.set_backlight_line(BacklightLine::Low, high),
            BACKLIGHT_MID_PIN => self.display.set_backlight_line(BacklightLine::Mid, high),
            BACKLIGHT_HIGH_PIN => self.display.set_backlight_line(BacklightLine::High, high),
            TOUCH_RESET_PIN => self.touch.set_reset_pin(high, context),
            SIDE_BUTTON_ENABLE_PIN => self.side_button.set_enable_pin(high, context),
            MOTOR_PIN => self.motor.set_pin_level(high),
            _ => {}
        }
    }

    /// Chip selects are active low; a part whose select lifts finishes its command.
    fn update_chip_selects(&mut self, context: &mut dyn DeviceContext) {
        let display_selected = !is_high(self.pin_levels, DISPLAY_CHIP_SELECT_PIN);
        let flash_selected = !is_high(self.pin_levels, FLASH_CHIP_SELECT_PIN);
        let was_display = matches!(self.spi_target, SpiTarget::Display | SpiTarget::Both);
        let was_flash = matches!(self.spi_target, SpiTarget::ExternalFlash | SpiTarget::Both);
        if was_display && !display_selected {
            self.display.finish_transmission();
        }
        if was_flash && !flash_selected {
            self.external_flash.finish_transmission(context.now());
        }
        self.spi_target = match (display_selected, flash_selected) {
            (false, false) => SpiTarget::Nothing,
            (true, false) => SpiTarget::Display,
            (false, true) => SpiTarget::ExternalFlash,
            (true, true) => SpiTarget::Both,
        };
    }

    fn i2c_device(&mut self, target: I2cTarget) -> &mut dyn I2cDevice {
        match target {
            I2cTarget::Touch => &mut self.touch,
            I2cTarget::Accelerometer => &mut self.accelerometer,
            I2cTarget::HeartRate => &mut self.heart_rate,
        }
    }
}

// ============================================================================
// Ports to the chip
// ============================================================================

impl BoardPorts for PineTimeBoard {
    /// A pin the chip does not drive reads its board pull-up, or low without one.
    fn pin_outputs_changed(&mut self, levels: u32, outputs: u32, context: &mut dyn DeviceContext) {
        let new_levels = (levels & outputs) | (BOARD_PULL_UPS & !outputs);
        let mut changed = (new_levels ^ self.pin_levels) & WATCHED_PINS;
        if self.announce_every_pin {
            self.announce_every_pin = false;
            changed = WATCHED_PINS;
        }
        self.pin_levels = new_levels;
        if changed == 0 {
            return;
        }

        let mut remaining = changed;
        while remaining != 0 {
            let pin = remaining.trailing_zeros() as u8;
            remaining &= remaining - 1;
            self.pin_changed(pin, is_high(new_levels, pin), context);
        }
        if changed & ((1 << DISPLAY_CHIP_SELECT_PIN) | (1 << FLASH_CHIP_SELECT_PIN)) != 0 {
            self.update_chip_selects(context);
        }
    }

    #[inline]
    fn spi_transfer(&mut self, instance: u8, byte: u8, context: &mut dyn DeviceContext) -> u8 {
        if instance != SPI_INSTANCE {
            return IDLE_BUS;
        }
        let answer = match self.spi_target {
            SpiTarget::Display => self.display.transfer(byte),
            SpiTarget::ExternalFlash => self.external_flash.transfer(byte, context.now()),
            SpiTarget::Nothing | SpiTarget::Both => IDLE_BUS,
        };
        if TRACING_SPI.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!(
                "{} spi {:?} {byte:02x} -> {answer:02x}",
                context.now(),
                self.spi_target
            );
        }
        answer
    }

    fn i2c_start(&mut self, instance: u8, address: u8, read: bool, context: &mut dyn DeviceContext) -> bool {
        self.i2c_target = match (instance, address) {
            (I2C_INSTANCE, TOUCH_ADDRESS) => Some(I2cTarget::Touch),
            (I2C_INSTANCE, ACCELEROMETER_ADDRESS) => Some(I2cTarget::Accelerometer),
            (I2C_INSTANCE, HEART_RATE_ADDRESS) => Some(I2cTarget::HeartRate),
            _ => None,
        };
        match self.i2c_target {
            Some(target) => {
                self.i2c_device(target).start(read, context);
                true
            }
            None => false,
        }
    }

    fn i2c_write(&mut self, instance: u8, byte: u8, context: &mut dyn DeviceContext) -> bool {
        match self.i2c_target {
            Some(target) if instance == I2C_INSTANCE => {
                self.i2c_device(target).write(byte, context);
                true
            }
            _ => false,
        }
    }

    fn i2c_read(&mut self, instance: u8, context: &mut dyn DeviceContext) -> u8 {
        match self.i2c_target {
            Some(target) if instance == I2C_INSTANCE => self.i2c_device(target).read(context),
            _ => IDLE_BUS,
        }
    }

    fn i2c_stop(&mut self, instance: u8, context: &mut dyn DeviceContext) {
        if instance != I2C_INSTANCE {
            return;
        }
        if let Some(target) = self.i2c_target.take() {
            self.i2c_device(target).stop(context);
        }
    }

    fn analog_input_volts(&self, channel: u8) -> Option<f32> {
        (channel == BATTERY_ANALOG_INPUT).then(|| self.battery.analog_input_volts())
    }

    fn handle_event(&mut self, key: EventKey, context: &mut dyn DeviceContext) {
        let Owner::Device(device) = key.owner else {
            return;
        };
        match device {
            TOUCH_DEVICE => self.touch.handle_event(key.token, context),
            ACCELEROMETER_DEVICE => self.accelerometer.handle_event(key.token, context),
            SIDE_BUTTON_DEVICE => self.side_button.handle_event(key.token, context),
            _ => {}
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestContext;
    use pinetime_core::time::cycles_from_milliseconds;

    /// All pins outputs, with the given ones low and every other one high.
    fn outputs_low(low_pins: &[u8]) -> (u32, u32) {
        let low = low_pins.iter().fold(0u32, |mask, pin| mask | (1 << pin));
        (!low, u32::MAX)
    }

    fn powered_board(context: &mut TestContext) -> PineTimeBoard {
        let mut board = PineTimeBoard::new(AccelerometerModel::Bma425);
        board.power_on(context);
        board
    }

    fn set_outputs_low(board: &mut PineTimeBoard, low_pins: &[u8], context: &mut TestContext) {
        let (levels, outputs) = outputs_low(low_pins);
        board.pin_outputs_changed(levels, outputs, context);
    }

    /// Sends one command and its data with the backlight at full.
    fn display_command(board: &mut PineTimeBoard, command: u8, data: &[u8], context: &mut TestContext) {
        let selected_for_command = [
            BACKLIGHT_HIGH_PIN,
            DISPLAY_CHIP_SELECT_PIN,
            DISPLAY_DATA_COMMAND_PIN,
        ];
        set_outputs_low(board, &selected_for_command, context);
        board.spi_transfer(SPI_INSTANCE, command, context);
        set_outputs_low(board, &[BACKLIGHT_HIGH_PIN, DISPLAY_CHIP_SELECT_PIN], context);
        for byte in data {
            board.spi_transfer(SPI_INSTANCE, *byte, context);
        }
        set_outputs_low(board, &[BACKLIGHT_HIGH_PIN], context);
    }

    fn flash_command(board: &mut PineTimeBoard, bytes: &[u8], context: &mut TestContext) -> Vec<u8> {
        set_outputs_low(board, &[FLASH_CHIP_SELECT_PIN], context);
        let answer = bytes
            .iter()
            .map(|byte| board.spi_transfer(SPI_INSTANCE, *byte, context))
            .collect();
        set_outputs_low(board, &[], context);
        answer
    }

    fn run_for(board: &mut PineTimeBoard, milliseconds: u64, context: &mut TestContext) {
        context.advance_milliseconds(milliseconds, |key, context| board.handle_event(key, context));
    }

    #[test]
    fn power_on_drives_the_idle_input_levels() {
        let mut context = TestContext::new();
        let _board = powered_board(&mut context);
        assert_eq!(context.driven_level(SIDE_BUTTON_SENSE_PIN), None);
        assert_eq!(context.driven_level(POWER_PRESENT_PIN), None);
        assert_eq!(context.driven_level(CHARGING_PIN), None);
        assert_eq!(context.driven_level(TOUCH_INTERRUPT_PIN), Some(true));
        assert_eq!(context.driven_level(ACCELEROMETER_INTERRUPT_PIN), Some(false));
        assert_eq!(PineTimeBoard::board_pull_ups(), 0x0208_1020);
    }

    #[test]
    fn undriven_chip_selects_leave_both_parts_deselected() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        board.pin_outputs_changed(0, 0, &mut context);
        assert_eq!(board.pin_levels(), BOARD_PULL_UPS);
        assert_eq!(board.spi_transfer(SPI_INSTANCE, RDID_COMMAND, &mut context), 0xFF);
    }

    const RDID_COMMAND: u8 = 0x9F;

    #[test]
    fn routes_spi_to_the_flash_while_its_select_is_low() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        let answer = flash_command(&mut board, &[RDID_COMMAND, 0, 0, 0], &mut context);
        assert_eq!(answer, vec![0xFF, 0x0B, 0x40, 0x16]);
    }

    #[test]
    fn returns_an_idle_bus_when_both_parts_are_selected() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        set_outputs_low(
            &mut board,
            &[FLASH_CHIP_SELECT_PIN, DISPLAY_CHIP_SELECT_PIN],
            &mut context,
        );
        board.spi_transfer(SPI_INSTANCE, RDID_COMMAND, &mut context);
        assert_eq!(board.spi_transfer(SPI_INSTANCE, 0, &mut context), 0xFF);
        assert_eq!(board.spi_transfer(1, 0, &mut context), 0xFF);
    }

    #[test]
    fn lifting_the_flash_select_finishes_its_command() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        flash_command(&mut board, &[0x06], &mut context);
        assert!(board.external_flash().write_enabled());

        flash_command(&mut board, &[0x20, 0x00, 0x10, 0x00], &mut context);
        let status = flash_command(&mut board, &[0x05, 0], &mut context);
        assert_eq!(status[1] & 0x01, 0x01);

        context.now += cycles_from_milliseconds(45);
        let status = flash_command(&mut board, &[0x05, 0], &mut context);
        assert_eq!(status[1] & 0x01, 0x00);
    }

    #[test]
    fn drives_the_display_through_its_select_dc_and_reset_pins() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        display_command(&mut board, 0x11, &[], &mut context);
        display_command(&mut board, 0x3A, &[0x55], &mut context);
        display_command(&mut board, 0x21, &[], &mut context);
        display_command(&mut board, 0x29, &[], &mut context);
        display_command(&mut board, 0x2A, &[0, 1, 0, 1], &mut context);
        display_command(&mut board, 0x2B, &[0, 0, 0, 0], &mut context);
        display_command(&mut board, 0x2C, &[0xF8, 0x00], &mut context);

        assert_eq!(board.display().shown_pixel(1, 0), 0xFF0000);
        assert_eq!(board.display().status_flags(), 10);

        set_outputs_low(&mut board, &[BACKLIGHT_HIGH_PIN, DISPLAY_RESET_PIN], &mut context);
        assert!(board.display().in_reset());
        set_outputs_low(&mut board, &[BACKLIGHT_HIGH_PIN], &mut context);
        assert!(!board.display().in_reset());
        assert!(board.display().sleeping());
    }

    #[test]
    fn the_backlight_pins_set_the_level_and_the_motor_runs_while_its_pin_is_low() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        assert_eq!(board.backlight_level(), 3);
        set_outputs_low(&mut board, &[BACKLIGHT_LOW_PIN], &mut context);
        assert_eq!(board.backlight_level(), 1);
        set_outputs_low(&mut board, &[BACKLIGHT_MID_PIN], &mut context);
        assert_eq!(board.backlight_level(), 2);
        set_outputs_low(&mut board, &[BACKLIGHT_HIGH_PIN, MOTOR_PIN], &mut context);
        assert_eq!(board.backlight_level(), 3);
        assert!(board.motor_running());
        set_outputs_low(&mut board, &[], &mut context);
        assert_eq!(board.backlight_level(), 0);
        assert!(!board.motor_running());
        assert_eq!(board.motor_starts(), 1);
    }

    #[test]
    fn the_side_button_reads_pressed_only_while_enabled() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        board.side_button_mut().press(&mut context);
        assert_eq!(context.driven_level(SIDE_BUTTON_SENSE_PIN), Some(false));

        board.pin_outputs_changed(
            1 << SIDE_BUTTON_ENABLE_PIN,
            1 << SIDE_BUTTON_ENABLE_PIN,
            &mut context,
        );
        assert_eq!(context.driven_level(SIDE_BUTTON_SENSE_PIN), Some(true));

        board.side_button_mut().press_and_release(&mut context);
        run_for(&mut board, 100, &mut context);
        assert!(!board.side_button().pressed());
        assert_eq!(context.driven_level(SIDE_BUTTON_SENSE_PIN), None);
    }

    #[test]
    fn routes_i2c_by_address_on_the_sensor_bus() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        for (address, register, expected) in [
            (TOUCH_ADDRESS, 0xA7, 0xB4),
            (ACCELEROMETER_ADDRESS, 0x00, 0x13),
            (HEART_RATE_ADDRESS, 0x00, 0x21),
        ] {
            assert!(board.i2c_start(I2C_INSTANCE, address, false, &mut context));
            assert!(board.i2c_write(I2C_INSTANCE, register, &mut context));
            assert!(board.i2c_start(I2C_INSTANCE, address, true, &mut context));
            assert_eq!(board.i2c_read(I2C_INSTANCE, &mut context), expected);
            board.i2c_stop(I2C_INSTANCE, &mut context);
        }

        assert!(!board.i2c_start(I2C_INSTANCE, 0x50, false, &mut context));
        assert!(!board.i2c_write(I2C_INSTANCE, 0, &mut context));
        assert_eq!(board.i2c_read(I2C_INSTANCE, &mut context), 0xFF);
        assert!(!board.i2c_start(0, TOUCH_ADDRESS, false, &mut context));
    }

    #[test]
    fn routes_touch_events_and_its_reset_pin() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        board.touch_mut().tap(10, 20, &mut context);
        run_for(&mut board, 100, &mut context);
        assert_eq!(board.touch().gesture(), crate::touch::Gesture::SingleTap);

        set_outputs_low(&mut board, &[], &mut context);
        set_outputs_low(&mut board, &[TOUCH_RESET_PIN], &mut context);
        assert!(board.touch().held_in_reset());
        set_outputs_low(&mut board, &[], &mut context);
        assert!(!board.touch().held_in_reset());
    }

    #[test]
    fn routes_accelerometer_events() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        for (register, value) in [(0x55, 0x00), (0x53, 0x0A), (0x56, 0x40)] {
            board.i2c_start(I2C_INSTANCE, ACCELEROMETER_ADDRESS, false, &mut context);
            board.i2c_write(I2C_INSTANCE, register, &mut context);
            board.i2c_write(I2C_INSTANCE, value, &mut context);
            board.i2c_stop(I2C_INSTANCE, &mut context);
        }
        board
            .accelerometer_mut()
            .trigger_feature_interrupt(0x40, &mut context);
        assert_eq!(context.driven_level(ACCELEROMETER_INTERRUPT_PIN), Some(true));
        run_for(&mut board, 1, &mut context);
        assert_eq!(context.driven_level(ACCELEROMETER_INTERRUPT_PIN), Some(false));
    }

    #[test]
    fn the_battery_sits_on_ain7_and_drives_the_charger_lines() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        board.battery_mut().set_percent(50, &mut context);
        board.battery_mut().set_charger_connected(true, &mut context);

        assert!((board.analog_input_volts(7).unwrap() - 1.8).abs() < 1e-6);
        assert_eq!(board.analog_input_volts(6), None);
        assert_eq!(board.battery().percent(), 50);
        assert!(board.battery().charger_connected());
        assert!(board.battery().charging());
        assert_eq!(context.driven_level(POWER_PRESENT_PIN), Some(false));
        assert_eq!(context.driven_level(CHARGING_PIN), Some(false));
    }

    #[test]
    fn the_window_status_reflects_the_parts() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        board.battery_mut().set_percent(50, &mut context);
        board.battery_mut().set_charger_connected(true, &mut context);
        board.side_button_mut().press(&mut context);
        board.accelerometer_mut().add_steps(12, &mut context);
        board.heart_rate_mut().set_heart_rate(90, context.now);

        assert!(board.side_button().pressed());
        assert!(board.battery().charger_connected() && board.battery().charging());
        assert_eq!(board.battery().percent(), 50);
        assert_eq!(board.accelerometer().steps(), 12);
        assert_eq!(board.heart_rate().heart_rate(), 90);
        assert!(!board.motor_running());
    }

    #[test]
    fn reset_reannounces_every_pin_on_the_next_update() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        display_command(&mut board, 0x11, &[], &mut context);
        board.external_flash_mut().contents_mut()[0] = 0x12;

        board.reset(&mut context);
        board.pin_outputs_changed(0, 0, &mut context);

        assert!(board.display().in_reset());
        assert!(board.touch().held_in_reset());
        assert_eq!(board.backlight_level(), 3);
        assert_eq!(board.external_flash().contents()[0], 0x12);
    }

    #[test]
    fn ignores_events_it_does_not_own() {
        let mut context = TestContext::new();
        let mut board = powered_board(&mut context);
        board.handle_event(EventKey::new(Owner::Peripheral(3), 0), &mut context);
        board.handle_event(EventKey::new(Owner::Device(MOTOR_DEVICE), 0), &mut context);
    }
}
