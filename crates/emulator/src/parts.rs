// SPDX-License-Identifier: GPL-3.0-only
//! Monitor commands for the watch parts, named after the Renode parts the window and tests address.

use std::path::PathBuf;

use pinetime_board::SwipeDirection;
use pinetime_core::time::nanoseconds_from_cycles;
use pinetime_nrf52832::ResetKind;

use crate::machine::Machine;
use crate::monitor::{parse_number, Reply};
use crate::sharing::{FrameFile, FrameSnapshot, StatusSnapshot};

pub const SCREEN_WIDTH: u16 = 240;
pub const SCREEN_HEIGHT: u16 = 240;

// ============================================================================
// Commands
// ============================================================================

/// Runs `command` on the part `name`, the last word of its Renode path.
pub fn run(machine: &mut Machine, name: &str, command: &str, arguments: &[&str]) -> Reply {
    match (name, command) {
        ("sideButton", "Press") => {
            machine.with_board(|board, context| board.side_button_mut().press(context))
        }
        ("sideButton", "Release") => {
            machine.with_board(|board, context| board.side_button_mut().release(context))
        }
        ("sideButton", "PressAndRelease") => {
            machine.with_board(|board, context| board.side_button_mut().press_and_release(context))
        }
        ("sideButton", "Hold") => {
            let milliseconds = (number(arguments, 0)? * 1000.0) as u64;
            machine.with_board(|board, context| board.side_button_mut().hold(milliseconds, context))
        }

        ("touch", "Touch") => point_command(machine, arguments, |touch, x, y, context| {
            touch.touch(x, y, context)
        })?,
        ("touch", "Move") => point_command(machine, arguments, |touch, x, y, context| {
            touch.move_finger(x, y, context)
        })?,
        ("touch", "MoveTo") => point_command(machine, arguments, |touch, x, y, context| {
            touch.move_to(x, y, context)
        })?,
        ("touch", "Tap") => point_command(machine, arguments, |touch, x, y, context| {
            touch.tap(x, y, context)
        })?,
        ("touch", "DoubleTap") => point_command(machine, arguments, |touch, x, y, context| {
            touch.double_tap(x, y, context)
        })?,
        ("touch", "LongPress") => point_command(machine, arguments, |touch, x, y, context| {
            touch.long_press(x, y, context)
        })?,
        ("touch", "Press") => machine.with_board(|board, context| board.touch_mut().press(context)),
        ("touch", "Release") => machine.with_board(|board, context| board.touch_mut().release(context)),
        ("touch", "Swipe") => {
            let text = argument(arguments, 0)?;
            let direction =
                SwipeDirection::parse(text).ok_or_else(|| format!("not a swipe direction: {text}"))?;
            machine.with_board(|board, context| board.touch_mut().swipe(direction, context))
        }

        ("battery", "Percent") if arguments.is_empty() => {
            return Ok(machine.board.battery().percent().to_string())
        }
        ("battery", "Percent") => {
            let percent = number(arguments, 0)? as i32;
            machine.with_board(|board, context| board.battery_mut().set_percent(percent, context))
        }
        ("battery", "Volts") if arguments.is_empty() => {
            return Ok(format!("{}", machine.board.battery().volts()))
        }
        ("battery", "Volts") => {
            let volts = number(arguments, 0)?;
            machine.with_board(|board, context| board.battery_mut().set_volts(volts, context))
        }
        ("battery", "ChargerConnected") if arguments.is_empty() => {
            return Ok(boolean_text(machine.board.battery().charger_connected()))
        }
        ("battery", "ChargerConnected") => {
            let connected = boolean(arguments, 0)?;
            machine.with_board(|board, context| board.battery_mut().set_charger_connected(connected, context))
        }
        ("battery", "ConnectCharger") => {
            machine.with_board(|board, context| board.battery_mut().connect_charger(context))
        }
        ("battery", "DisconnectCharger") => {
            machine.with_board(|board, context| board.battery_mut().disconnect_charger(context))
        }

        ("accelerometer", "AddSteps") => {
            let steps = number(arguments, 0)? as u32;
            machine.with_board(|board, context| board.accelerometer_mut().add_steps(steps, context))
        }
        ("accelerometer", "SetSteps") => {
            let steps = number(arguments, 0)? as u32;
            machine.with_board(|board, context| board.accelerometer_mut().set_steps(steps, context))
        }
        ("accelerometer", "SetAcceleration") => {
            let x = number(arguments, 0)? as i32;
            let y = number(arguments, 1)? as i32;
            let z = number(arguments, 2)? as i32;
            machine.board.accelerometer_mut().set_acceleration(x, y, z);
        }
        ("accelerometer", "SetTemperature") => {
            let celsius = number(arguments, 0)?;
            machine.board.accelerometer_mut().set_temperature(celsius);
        }

        ("heartRateSensor", "SetHeartRate") => {
            let beats_per_minute = number(arguments, 0)? as u32;
            let now = machine.now();
            machine
                .board
                .heart_rate_mut()
                .set_heart_rate(beats_per_minute, now);
        }
        ("heartRateSensor", "SetAmbientLight") => {
            let level = number(arguments, 0)? as u32;
            machine.board.heart_rate_mut().set_ambient_light(level);
        }

        ("powerClock", "PowerCycle") => machine.power_on(),
        ("powerClock", "ResetMachine") => machine.reset_chip(ResetKind::Pin),

        ("lcd", "Sleeping") => return Ok(boolean_text(machine.board.display().sleeping())),
        ("lcd", "DisplayOn") => return Ok(boolean_text(machine.board.display().display_on())),
        ("lcd", "Inverted") => return Ok(boolean_text(machine.board.display().inverted())),
        ("lcd", "InReset") => return Ok(boolean_text(machine.board.display().in_reset())),
        ("lcd", "BacklightLevel") => return Ok(machine.board.display().backlight_level().to_string()),
        ("sideButton", "Pressed") => return Ok(boolean_text(machine.board.side_button().pressed())),
        ("vibrationMotor", "State") => return Ok(boolean_text(machine.board.motor_running())),
        ("lcd", "SaveScreenshot") => {
            let path = path_argument(argument(arguments, 0)?);
            machine
                .board
                .display()
                .save_screenshot(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }

        ("externalFlashMemory", "LoadBinary") | ("externalFlash", "LoadBinary") => {
            let path = path_argument(argument(arguments, 0)?);
            let offset = arguments
                .get(1)
                .map(|text| parse_number(text))
                .transpose()?
                .unwrap_or(0) as usize;
            let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            load_external_flash(machine, offset, &bytes)?;
        }
        ("externalFlash", "DeepPoweredDown") => {
            return Ok(boolean_text(machine.board.external_flash().deep_powered_down()))
        }
        ("externalFlashMemory", "SaveBinary") | ("externalFlash", "SaveBinary") => {
            let path = path_argument(argument(arguments, 0)?);
            machine
                .board
                .external_flash()
                .save_image_file(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }

        _ => return Err(format!("unknown command: {name} {command}")),
    }
    Ok(String::new())
}

fn point_command(
    machine: &mut Machine,
    arguments: &[&str],
    change: impl FnOnce(&mut pinetime_board::Touch, i32, i32, &mut pinetime_core::QueuedContext),
) -> Result<(), String> {
    let x = number(arguments, 0)? as i32;
    let y = number(arguments, 1)? as i32;
    machine.with_board(|board, context| change(board.touch_mut(), x, y, context));
    Ok(())
}

// ============================================================================
// External flash
// ============================================================================

pub fn load_external_flash(machine: &mut Machine, offset: usize, bytes: &[u8]) -> Result<(), String> {
    let contents = machine.board.external_flash_mut().contents_mut();
    let end = offset
        .checked_add(bytes.len())
        .filter(|end| *end <= contents.len());
    let end = end.ok_or_else(|| {
        format!(
            "{} bytes at {offset:#x} do not fit in the external flash",
            bytes.len()
        )
    })?;
    contents[offset..end].copy_from_slice(bytes);
    Ok(())
}

pub fn read_external_flash(machine: &Machine, offset: usize, count: usize) -> Result<Vec<u8>, String> {
    let contents = machine.board.external_flash().contents();
    contents
        .get(offset..offset + count)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| format!("{count} bytes at {offset:#x} run past the external flash"))
}

// ============================================================================
// Window sharing
// ============================================================================

pub fn visible_size(_machine: &Machine) -> (u16, u16) {
    (SCREEN_WIDTH, SCREEN_HEIGHT)
}

pub fn write_frame(machine: &Machine, file: &mut FrameFile) -> std::io::Result<()> {
    let display = machine.board.display();
    let mut pixels = vec![0u8; SCREEN_WIDTH as usize * SCREEN_HEIGHT as usize * 3];
    display.visible_frame_rgb888(&mut pixels);
    let frame = FrameSnapshot {
        count: display.frame_generation() as u32,
        width: SCREEN_WIDTH,
        height: SCREEN_HEIGHT,
        sleeping: display.sleeping(),
        display_on: display.display_on(),
        in_reset: display.in_reset(),
        inverted: display.inverted(),
        backlight_level: display.backlight_level() as u32,
        rgb_pixels: &pixels,
    };
    file.write(&frame, false)
}

pub fn status(machine: &Machine, paused: bool) -> StatusSnapshot {
    let board = &machine.board;
    StatusSnapshot {
        motor_running: board.motor_running(),
        button_pressed: board.side_button().pressed(),
        charger_connected: board.battery().charger_connected(),
        charging: board.battery().charging(),
        paused,
        motor_starts: board.motor_starts(),
        battery_percent: board.battery().percent(),
        battery_volts: board.battery().volts(),
        emulated_microseconds: nanoseconds_from_cycles(machine.now()) / 1000,
        steps: board.accelerometer().steps(),
    }
}

// ============================================================================
// Arguments
// ============================================================================

fn argument<'a>(arguments: &[&'a str], index: usize) -> Result<&'a str, String> {
    arguments
        .get(index)
        .copied()
        .ok_or_else(|| format!("missing argument {}", index + 1))
}

fn number(arguments: &[&str], index: usize) -> Result<f64, String> {
    let text = argument(arguments, index)?;
    text.parse::<f64>()
        .or_else(|_| parse_number(text).map(f64::from))
        .map_err(|_| format!("not a number: {text}"))
}

fn boolean(arguments: &[&str], index: usize) -> Result<bool, String> {
    match argument(arguments, index)?.to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(format!("not true or false: {other}")),
    }
}

fn boolean_text(value: bool) -> String {
    if value { "True" } else { "False" }.to_string()
}

fn path_argument(text: &str) -> PathBuf {
    PathBuf::from(text.strip_prefix('@').unwrap_or(text))
}
