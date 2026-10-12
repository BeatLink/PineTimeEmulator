// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime's 240x240 screen and its Sitronix ST7789 controller, with data/command and reset pins.

use std::io;
use std::path::Path;

use crate::backlight::{Backlight, BacklightLine};
use crate::png;

// ============================================================================
// Sizes and commands
// ============================================================================

pub const SCREEN_WIDTH: usize = 240;
pub const SCREEN_HEIGHT: usize = 240;
pub const MEMORY_COLUMNS: usize = 240;
pub const MEMORY_ROWS: usize = 320;

const NOP: u8 = 0x00;
const SWRESET: u8 = 0x01;
const RDDID: u8 = 0x04;
const RDDST: u8 = 0x09;
const RDDPM: u8 = 0x0A;
const RDDMADCTL: u8 = 0x0B;
const RDDCOLMOD: u8 = 0x0C;
const RDDIM: u8 = 0x0D;
const RDDSM: u8 = 0x0E;
const RDDSDR: u8 = 0x0F;
const SLPIN: u8 = 0x10;
const SLPOUT: u8 = 0x11;
const PTLON: u8 = 0x12;
const NORON: u8 = 0x13;
const INVOFF: u8 = 0x20;
const INVON: u8 = 0x21;
const DISPOFF: u8 = 0x28;
const DISPON: u8 = 0x29;
const CASET: u8 = 0x2A;
const RASET: u8 = 0x2B;
const RAMWR: u8 = 0x2C;
const VSCRDEF: u8 = 0x33;
const TEOFF: u8 = 0x34;
const TEON: u8 = 0x35;
const MADCTL: u8 = 0x36;
const VSCSAD: u8 = 0x37;
const IDMOFF: u8 = 0x38;
const IDMON: u8 = 0x39;
const COLMOD: u8 = 0x3A;
const RAMWRC: u8 = 0x3C;
const RDID1: u8 = 0xDA;
const RDID2: u8 = 0xDB;
const RDID3: u8 = 0xDC;

const COLOUR_FORMAT_12_BIT: u8 = 3;
const COLOUR_FORMAT_16_BIT: u8 = 5;
const DEFAULT_PIXEL_FORMAT: u8 = 0x66;

const MADCTL_ROW_MIRROR: u8 = 0x80;
const MADCTL_COLUMN_MIRROR: u8 = 0x40;
const MADCTL_ROW_COLUMN_EXCHANGE: u8 = 0x20;
const MADCTL_BLUE_GREEN_RED: u8 = 0x08;

const MANUFACTURER_ID: u8 = 0x85;
const VERSION_ID: u8 = 0x85;
const MODULE_ID: u8 = 0x52;

const STORED_PARAMETERS: usize = 6;
const LONGEST_READ: usize = 5;

/// Bits of `status_flags`, as the window's frame file carries them.
pub const FLAG_SLEEPING: u32 = 1;
pub const FLAG_DISPLAY_ON: u32 = 2;
pub const FLAG_IN_RESET: u32 = 4;
pub const FLAG_INVERTED: u32 = 8;

// ============================================================================
// Controller state
// ============================================================================

pub struct St7789 {
    frame_memory: Box<[u16]>,
    data_command_pin_high: bool,
    reset_pin_low: bool,
    backlight: Backlight,
    frame_generation: u64,

    current_command: Option<u8>,
    parameters: [u8; STORED_PARAMETERS],
    parameter_count: usize,
    read_bytes: [u8; LONGEST_READ],
    read_position: usize,
    read_length: usize,
    pixel_bytes: [u8; 3],
    pixel_byte_count: usize,

    sleeping: bool,
    display_on: bool,
    inversion_on: bool,
    idle_mode: bool,
    partial_mode: bool,
    tearing_effect_on: bool,
    memory_access_control: u8,
    interface_pixel_format: u8,

    column_start: usize,
    column_end: usize,
    row_start: usize,
    row_end: usize,
    write_column: usize,
    write_row: usize,

    scroll_mode: bool,
    top_fixed_area: usize,
    scroll_area: usize,
    bottom_fixed_area: usize,
    scroll_start: usize,
}

impl Default for St7789 {
    fn default() -> St7789 {
        St7789::new()
    }
}

impl St7789 {
    pub fn new() -> St7789 {
        let mut display = St7789 {
            frame_memory: vec![0; MEMORY_COLUMNS * MEMORY_ROWS].into_boxed_slice(),
            data_command_pin_high: false,
            reset_pin_low: false,
            backlight: Backlight::new(),
            frame_generation: 0,
            current_command: None,
            parameters: [0; STORED_PARAMETERS],
            parameter_count: 0,
            read_bytes: [0; LONGEST_READ],
            read_position: 0,
            read_length: 0,
            pixel_bytes: [0; 3],
            pixel_byte_count: 0,
            sleeping: true,
            display_on: false,
            inversion_on: false,
            idle_mode: false,
            partial_mode: false,
            tearing_effect_on: false,
            memory_access_control: 0,
            interface_pixel_format: DEFAULT_PIXEL_FORMAT,
            column_start: 0,
            column_end: MEMORY_COLUMNS - 1,
            row_start: 0,
            row_end: MEMORY_ROWS - 1,
            write_column: 0,
            write_row: 0,
            scroll_mode: false,
            top_fixed_area: 0,
            scroll_area: MEMORY_ROWS,
            bottom_fixed_area: 0,
            scroll_start: 0,
        };
        display.reset_controller();
        display
    }

    /// Powers the screen up afresh: frame memory cleared, pins idle and the backlight full.
    pub fn reset(&mut self) {
        self.frame_memory.fill(0);
        self.data_command_pin_high = false;
        self.reset_pin_low = false;
        self.backlight = Backlight::new();
        self.reset_controller();
    }

    // ========================================================================
    // Pins
    // ========================================================================

    pub fn set_data_command_pin(&mut self, high: bool) {
        self.data_command_pin_high = high;
    }

    /// The reset pin is active low; pulling it low restores the controller defaults.
    pub fn set_reset_pin(&mut self, high: bool) {
        if !high && !self.reset_pin_low {
            self.reset_controller();
        }
        self.reset_pin_low = !high;
        self.mark_changed();
    }

    pub fn set_backlight_line(&mut self, line: BacklightLine, pin_high: bool) {
        self.backlight.set_line(line, pin_high);
        self.mark_changed();
    }

    // ========================================================================
    // SPI
    // ========================================================================

    /// Takes one byte from the bus: a command while data/command is low, data while it is high.
    #[inline]
    pub fn transfer(&mut self, byte: u8) -> u8 {
        if self.reset_pin_low {
            return 0;
        }
        if !self.data_command_pin_high {
            self.start_command(byte);
            return 0;
        }
        if self.read_position < self.read_length {
            let value = self.read_bytes[self.read_position];
            self.read_position += 1;
            return value;
        }
        match self.current_command {
            Some(RAMWR) | Some(RAMWRC) => self.receive_pixel_byte(byte),
            Some(command) => self.receive_parameter(command, byte),
            None => {}
        }
        0
    }

    /// The chip select lifting changes nothing, since commands and pixels are whole bytes here.
    pub fn finish_transmission(&mut self) {}

    // ========================================================================
    // State for the emulator and tests
    // ========================================================================

    pub fn sleeping(&self) -> bool {
        self.sleeping
    }

    pub fn display_on(&self) -> bool {
        self.display_on
    }

    /// True after INVON; the PineTime glass shows true colours only in this state.
    pub fn inverted(&self) -> bool {
        self.inversion_on
    }

    pub fn in_reset(&self) -> bool {
        self.reset_pin_low
    }

    pub fn scrolling(&self) -> bool {
        self.scroll_mode
    }

    pub fn scroll_start(&self) -> usize {
        self.scroll_start
    }

    pub fn top_fixed_area(&self) -> usize {
        self.top_fixed_area
    }

    pub fn scroll_area(&self) -> usize {
        self.scroll_area
    }

    pub fn bottom_fixed_area(&self) -> usize {
        self.bottom_fixed_area
    }

    pub fn memory_access_control(&self) -> u8 {
        self.memory_access_control
    }

    pub fn pixel_format_setting(&self) -> u8 {
        self.interface_pixel_format
    }

    pub fn tearing_effect_on(&self) -> bool {
        self.tearing_effect_on
    }

    /// 0 is off, 1 low, 2 mid, 3 full.
    pub fn backlight_level(&self) -> u8 {
        self.backlight.level()
    }

    /// Overrides the backlight level until one of its pins next changes.
    pub fn set_backlight_level(&mut self, level: u8) {
        self.backlight.set_level(level);
        self.mark_changed();
    }

    /// A number that changes whenever anything the glass shows may have changed.
    pub fn frame_generation(&self) -> u64 {
        self.frame_generation
    }

    /// The flags of the window's frame file: sleeping, display on, in reset and inverted.
    pub fn status_flags(&self) -> u32 {
        (self.sleeping as u32 * FLAG_SLEEPING)
            | (self.display_on as u32 * FLAG_DISPLAY_ON)
            | (self.reset_pin_low as u32 * FLAG_IN_RESET)
            | (self.inversion_on as u32 * FLAG_INVERTED)
    }

    /// The RGB565 value stored in frame memory at a physical column and row.
    pub fn memory_pixel(&self, column: usize, row: usize) -> u16 {
        assert!(
            column < MEMORY_COLUMNS && row < MEMORY_ROWS,
            "outside the 240x320 frame memory"
        );
        self.frame_memory[row * MEMORY_COLUMNS + column]
    }

    /// The colour the glass shows at (x, y), as 0xRRGGBB.
    pub fn shown_pixel(&self, x: usize, y: usize) -> u32 {
        assert!(
            x < SCREEN_WIDTH && y < SCREEN_HEIGHT,
            "outside the 240x240 screen"
        );
        if self.blank() {
            return 0;
        }
        self.shown_colour(self.shown_colour_565(x, self.shown_memory_row(y)))
    }

    /// Fills `rgb` with the 240x240 image the glass shows, three bytes a pixel.
    pub fn visible_frame_rgb888(&self, rgb: &mut [u8]) {
        assert_eq!(
            rgb.len(),
            SCREEN_WIDTH * SCREEN_HEIGHT * 3,
            "the buffer is not one frame"
        );
        if self.blank() {
            rgb.fill(0);
            return;
        }
        for (y, line) in rgb
            .as_chunks_mut::<{ SCREEN_WIDTH * 3 }>()
            .0
            .iter_mut()
            .enumerate()
        {
            let memory_row = self.shown_memory_row(y);
            for (x, pixel) in line.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let colour = self.shown_colour(self.shown_colour_565(x, memory_row));
                pixel[0] = (colour >> 16) as u8;
                pixel[1] = (colour >> 8) as u8;
                pixel[2] = colour as u8;
            }
        }
    }

    /// Fills `pixels` with the 240x240 image the glass shows as RGB565, before backlight dimming.
    pub fn visible_frame_rgb565(&self, pixels: &mut [u16]) {
        assert_eq!(
            pixels.len(),
            SCREEN_WIDTH * SCREEN_HEIGHT,
            "the buffer is not one frame"
        );
        if self.blank() {
            pixels.fill(0);
            return;
        }
        for (y, line) in pixels.as_chunks_mut::<SCREEN_WIDTH>().0.iter_mut().enumerate() {
            let memory_row = self.shown_memory_row(y);
            for (x, pixel) in line.iter_mut().enumerate() {
                *pixel = self.shown_colour_565(x, memory_row);
            }
        }
    }

    pub fn visible_frame_rgb888_vec(&self) -> Vec<u8> {
        let mut rgb = vec![0; SCREEN_WIDTH * SCREEN_HEIGHT * 3];
        self.visible_frame_rgb888(&mut rgb);
        rgb
    }

    pub fn save_screenshot(&self, path: &Path) -> io::Result<()> {
        let image = png::encode_rgb(
            SCREEN_WIDTH as u32,
            SCREEN_HEIGHT as u32,
            &self.visible_frame_rgb888_vec(),
        );
        std::fs::write(path, image)
    }

    // ========================================================================
    // Commands
    // ========================================================================

    fn start_command(&mut self, command: u8) {
        self.current_command = Some(command);
        self.parameter_count = 0;
        self.read_position = 0;
        self.read_length = 0;
        self.pixel_byte_count = 0;

        match command {
            NOP => {}
            SWRESET => self.reset_controller(),
            SLPIN => self.sleeping = true,
            SLPOUT => self.sleeping = false,
            PTLON => {
                self.partial_mode = true;
                self.scroll_mode = false;
            }
            NORON => {
                self.partial_mode = false;
                self.scroll_mode = false;
            }
            INVOFF => self.inversion_on = false,
            INVON => self.inversion_on = true,
            DISPOFF => self.display_on = false,
            DISPON => self.display_on = true,
            TEOFF => self.tearing_effect_on = false,
            IDMOFF => self.idle_mode = false,
            IDMON => self.idle_mode = true,
            RAMWR => {
                self.write_column = self.column_start;
                self.write_row = self.row_start;
            }
            RDDID | RDDST | RDDPM | RDDMADCTL | RDDCOLMOD | RDDIM | RDDSM | RDDSDR | RDID1 | RDID2
            | RDID3 => self.queue_read_bytes(command),
            _ => {}
        }
        self.mark_changed();
    }

    /// Panel tuning commands such as porch, gate and gamma settings take parameters and change nothing.
    fn receive_parameter(&mut self, command: u8, byte: u8) {
        if self.parameter_count < STORED_PARAMETERS {
            self.parameters[self.parameter_count] = byte;
        }
        self.parameter_count = self.parameter_count.saturating_add(1);

        match (command, self.parameter_count) {
            (CASET, 4) => {
                self.column_start = self.parameter_word(0);
                self.column_end = self.parameter_word(2);
            }
            (RASET, 4) => {
                self.row_start = self.parameter_word(0);
                self.row_end = self.parameter_word(2);
            }
            (VSCRDEF, 6) => {
                self.top_fixed_area = self.parameter_word(0);
                self.scroll_area = self.parameter_word(2);
                self.bottom_fixed_area = self.parameter_word(4);
            }
            (VSCSAD, 2) => {
                self.scroll_start = self.parameter_word(0);
                self.scroll_mode = true;
                self.mark_changed();
            }
            (TEON, 1) => self.tearing_effect_on = true,
            (MADCTL, 1) => {
                self.memory_access_control = byte;
                self.mark_changed();
            }
            (COLMOD, 1) => self.interface_pixel_format = byte,
            _ => {}
        }
    }

    fn parameter_word(&self, first_index: usize) -> usize {
        ((self.parameters[first_index] as usize) << 8) | self.parameters[first_index + 1] as usize
    }

    // ========================================================================
    // Reads
    // ========================================================================

    fn queue_read_bytes(&mut self, command: u8) {
        match command {
            RDDID => self.queue_bytes(&[0x00, MANUFACTURER_ID, VERSION_ID, MODULE_ID]),
            RDDST => self.queue_bytes(&[
                0x00,
                self.status_first_byte(),
                self.status_second_byte(),
                self.status_third_byte(),
                0x00,
            ]),
            RDDPM => self.queue_bytes(&[self.power_mode_byte()]),
            RDDMADCTL => self.queue_bytes(&[self.memory_access_control]),
            RDDCOLMOD => self.queue_bytes(&[self.interface_pixel_format]),
            RDDIM => self.queue_bytes(&[(self.scroll_mode as u8 * 0x80) | (self.inversion_on as u8 * 0x20)]),
            RDDSM => self.queue_bytes(&[self.tearing_effect_on as u8 * 0x80]),
            RDDSDR => self.queue_bytes(&[0x00]),
            RDID1 => self.queue_bytes(&[MANUFACTURER_ID]),
            RDID2 => self.queue_bytes(&[VERSION_ID]),
            RDID3 => self.queue_bytes(&[MODULE_ID]),
            _ => {}
        }
    }

    fn queue_bytes(&mut self, bytes: &[u8]) {
        self.read_bytes[..bytes.len()].copy_from_slice(bytes);
        self.read_position = 0;
        self.read_length = bytes.len();
    }

    fn power_mode_byte(&self) -> u8 {
        let mut value = 0;
        if !self.sleeping {
            value |= 0x80 | 0x10;
        }
        if self.idle_mode {
            value |= 0x40;
        }
        if self.partial_mode {
            value |= 0x20;
        } else {
            value |= 0x08;
        }
        if self.display_on {
            value |= 0x04;
        }
        value
    }

    fn status_first_byte(&self) -> u8 {
        (if self.sleeping { 0 } else { 0x80 }) | ((self.memory_access_control & 0xFC) >> 1)
    }

    fn status_second_byte(&self) -> u8 {
        let mut value = (self.interface_pixel_format & 0x07) << 4;
        if self.idle_mode {
            value |= 0x08;
        }
        if self.partial_mode {
            value |= 0x04;
        } else {
            value |= 0x01;
        }
        if !self.sleeping {
            value |= 0x02;
        }
        value
    }

    fn status_third_byte(&self) -> u8 {
        (self.scroll_mode as u8 * 0x80)
            | (self.inversion_on as u8 * 0x20)
            | (self.display_on as u8 * 0x04)
            | (self.tearing_effect_on as u8 * 0x02)
    }

    // ========================================================================
    // Frame memory writes
    // ========================================================================

    /// The 18-bit and 24-bit formats send one byte per colour and keep its top bits.
    #[inline]
    fn receive_pixel_byte(&mut self, byte: u8) {
        self.pixel_bytes[self.pixel_byte_count] = byte;
        self.pixel_byte_count += 1;
        let [first, second, third] = self.pixel_bytes;

        match self.interface_pixel_format & 0x07 {
            COLOUR_FORMAT_16_BIT => {
                if self.pixel_byte_count == 2 {
                    self.pixel_byte_count = 0;
                    self.store_pixel(((first as u16) << 8) | second as u16);
                }
            }
            COLOUR_FORMAT_12_BIT => {
                if self.pixel_byte_count == 3 {
                    self.pixel_byte_count = 0;
                    self.store_pixel(rgb444_to_rgb565(first >> 4, first & 0xF, second >> 4));
                    self.store_pixel(rgb444_to_rgb565(second & 0xF, third >> 4, third & 0xF));
                }
            }
            _ => {
                if self.pixel_byte_count == 3 {
                    self.pixel_byte_count = 0;
                    let red = (first as u16 >> 3) << 11;
                    let green = (second as u16 >> 2) << 5;
                    let blue = third as u16 >> 3;
                    self.store_pixel(red | green | blue);
                }
            }
        }
    }

    #[inline]
    fn store_pixel(&mut self, colour: u16) {
        let exchanged = self.memory_access_control & MADCTL_ROW_COLUMN_EXCHANGE != 0;
        let (logical_width, logical_height) = if exchanged {
            (MEMORY_ROWS, MEMORY_COLUMNS)
        } else {
            (MEMORY_COLUMNS, MEMORY_ROWS)
        };

        if self.write_column < logical_width && self.write_row < logical_height {
            let (mut column, mut row) = if exchanged {
                (self.write_row, self.write_column)
            } else {
                (self.write_column, self.write_row)
            };
            if self.memory_access_control & MADCTL_COLUMN_MIRROR != 0 {
                column = MEMORY_COLUMNS - 1 - column;
            }
            if self.memory_access_control & MADCTL_ROW_MIRROR != 0 {
                row = MEMORY_ROWS - 1 - row;
            }
            self.frame_memory[row * MEMORY_COLUMNS + column] = colour;
            self.frame_generation = self.frame_generation.wrapping_add(1);
        }

        self.advance_write_pointer();
    }

    #[inline]
    fn advance_write_pointer(&mut self) {
        self.write_column += 1;
        if self.write_column <= self.column_end {
            return;
        }
        self.write_column = self.column_start;
        self.write_row += 1;
        if self.write_row > self.row_end {
            self.write_row = self.row_start;
        }
    }

    // ========================================================================
    // What the glass shows
    // ========================================================================

    fn blank(&self) -> bool {
        self.reset_pin_low || self.sleeping || !self.display_on || self.backlight.level() == 0
    }

    /// The colour at a screen column and memory row after inversion and the colour order bit.
    fn shown_colour_565(&self, x: usize, memory_row: usize) -> u16 {
        let mut colour = self.frame_memory[memory_row * MEMORY_COLUMNS + x];
        if !self.inversion_on {
            colour = !colour;
        }
        if self.memory_access_control & MADCTL_BLUE_GREEN_RED != 0 {
            colour = (colour & 0x07E0) | (colour >> 11) | ((colour & 0x1F) << 11);
        }
        colour
    }

    /// Widens an RGB565 colour to 0xRRGGBB and dims it by the backlight.
    fn shown_colour(&self, colour: u16) -> u32 {
        let red = ((colour >> 11) & 0x1F) as u32;
        let green = ((colour >> 5) & 0x3F) as u32;
        let blue = (colour & 0x1F) as u32;
        let red = self.backlight.dim((red << 3) | (red >> 2));
        let green = self.backlight.dim((green << 2) | (green >> 4));
        let blue = self.backlight.dim((blue << 3) | (blue >> 2));
        (red << 16) | (green << 8) | blue
    }

    fn shown_memory_row(&self, screen_line: usize) -> usize {
        let (top, area) = (self.top_fixed_area as i64, self.scroll_area as i64);
        let line = screen_line as i64;
        if !self.scroll_mode || area <= 0 || line < top || line >= top + area {
            return screen_line;
        }
        let offset = (line - top) + (self.scroll_start as i64 - top);
        let wrapped = offset.rem_euclid(area);
        ((top + wrapped) as usize).min(MEMORY_ROWS - 1)
    }

    // ========================================================================
    // Controller reset
    // ========================================================================

    /// Both the reset pin and SWRESET restore these defaults and leave frame memory alone.
    fn reset_controller(&mut self) {
        self.current_command = None;
        self.parameter_count = 0;
        self.read_position = 0;
        self.read_length = 0;
        self.pixel_byte_count = 0;

        self.sleeping = true;
        self.display_on = false;
        self.inversion_on = false;
        self.idle_mode = false;
        self.partial_mode = false;
        self.tearing_effect_on = false;
        self.memory_access_control = 0;
        self.interface_pixel_format = DEFAULT_PIXEL_FORMAT;

        self.column_start = 0;
        self.column_end = MEMORY_COLUMNS - 1;
        self.row_start = 0;
        self.row_end = MEMORY_ROWS - 1;
        self.write_column = 0;
        self.write_row = 0;

        self.scroll_mode = false;
        self.top_fixed_area = 0;
        self.scroll_area = MEMORY_ROWS;
        self.bottom_fixed_area = 0;
        self.scroll_start = 0;
        self.mark_changed();
    }

    fn mark_changed(&mut self) {
        self.frame_generation = self.frame_generation.wrapping_add(1);
    }
}

fn rgb444_to_rgb565(red: u8, green: u8, blue: u8) -> u16 {
    let (red, green, blue) = (red as u16, green as u16, blue as u16);
    (((red << 1) | (red >> 3)) << 11) | (((green << 2) | (green >> 2)) << 5) | ((blue << 1) | (blue >> 3))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: u32 = 0x000000;
    const RED: u32 = 0xFF0000;
    const GREEN: u32 = 0x00FF00;
    const BLUE: u32 = 0x0000FF;
    const CYAN: u32 = 0x00FFFF;

    fn send_command(display: &mut St7789, command: u8) {
        display.set_data_command_pin(false);
        display.transfer(command);
    }

    fn send_data(display: &mut St7789, bytes: &[u8]) {
        display.set_data_command_pin(true);
        for byte in bytes {
            display.transfer(*byte);
        }
    }

    fn initialised_screen() -> St7789 {
        let mut display = St7789::new();
        send_command(&mut display, SWRESET);
        send_command(&mut display, SLPOUT);
        send_command(&mut display, COLMOD);
        send_data(&mut display, &[0x55]);
        send_command(&mut display, MADCTL);
        send_data(&mut display, &[0x00]);
        send_command(&mut display, 0xB2);
        send_data(&mut display, &[0x0C, 0x0C, 0x00, 0x33, 0x33]);
        send_command(&mut display, 0xBB);
        send_data(&mut display, &[0x35]);
        send_command(&mut display, INVON);
        send_command(&mut display, NORON);
        send_command(&mut display, DISPON);
        display
    }

    fn set_window(display: &mut St7789, first_column: u8, last_column: u8, first_row: u8, last_row: u8) {
        send_command(display, CASET);
        send_data(display, &[0, first_column, 0, last_column]);
        send_command(display, RASET);
        send_data(display, &[0, first_row, 0, last_row]);
    }

    fn write_pixels(display: &mut St7789, bytes: &[u8]) {
        send_command(display, RAMWR);
        send_data(display, bytes);
    }

    #[test]
    fn shows_a_filled_window_after_the_init_sequence() {
        let mut display = initialised_screen();
        assert!(!display.sleeping());
        assert!(display.display_on());
        assert!(display.inverted());

        set_window(&mut display, 10, 12, 20, 21);
        write_pixels(
            &mut display,
            &[
                0xF8, 0x00, 0xF8, 0x00, 0xF8, 0x00, 0x07, 0xE0, 0x07, 0xE0, 0x07, 0xE0, 0x00, 0x1F,
            ],
        );

        assert_eq!(display.shown_pixel(10, 20), BLUE);
        assert_eq!(display.shown_pixel(12, 20), RED);
        assert_eq!(display.shown_pixel(10, 21), GREEN);
        assert_eq!(display.shown_pixel(12, 21), GREEN);
        assert_eq!(display.shown_pixel(13, 20), BLACK);
        assert_eq!(display.shown_pixel(9, 21), BLACK);
    }

    #[test]
    fn shows_inverted_colours_without_invon() {
        let mut display = initialised_screen();
        set_window(&mut display, 0, 0, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00]);
        assert_eq!(display.shown_pixel(0, 0), RED);

        send_command(&mut display, INVOFF);

        assert!(!display.inverted());
        assert_eq!(display.shown_pixel(0, 0), CYAN);
        assert_eq!(display.shown_pixel(1, 0), 0xFFFFFF);
    }

    #[test]
    fn scrolls_the_middle_area_and_keeps_the_fixed_top() {
        let mut display = initialised_screen();
        set_window(&mut display, 0, 0, 1, 1);
        write_pixels(&mut display, &[0x00, 0x1F]);
        set_window(&mut display, 0, 0, 5, 5);
        write_pixels(&mut display, &[0xF8, 0x00]);
        send_command(&mut display, CASET);
        send_data(&mut display, &[0x00, 0x00, 0x00, 0x00]);
        send_command(&mut display, RASET);
        send_data(&mut display, &[0x01, 0x3F, 0x01, 0x3F]);
        write_pixels(&mut display, &[0x07, 0xE0]);

        send_command(&mut display, VSCRDEF);
        send_data(&mut display, &[0x00, 0x02, 0x01, 0x3E, 0x00, 0x00]);
        send_command(&mut display, VSCSAD);
        send_data(&mut display, &[0x00, 0x04]);

        assert_eq!(display.scroll_start(), 4);
        assert_eq!(display.top_fixed_area(), 2);
        assert_eq!(display.scroll_area(), 318);
        assert_eq!(display.shown_pixel(0, 1), BLUE);
        assert_eq!(display.shown_pixel(0, 3), RED);
        assert_eq!(display.shown_pixel(0, 5), BLACK);

        send_command(&mut display, VSCSAD);
        send_data(&mut display, &[0x01, 0x3F]);

        assert_eq!(display.shown_pixel(0, 1), BLUE);
        assert_eq!(display.shown_pixel(0, 2), GREEN);

        send_command(&mut display, NORON);

        assert!(!display.scrolling());
        assert_eq!(display.shown_pixel(0, 5), RED);
    }

    #[test]
    fn places_pixels_by_madctl_mirror_and_exchange_bits() {
        let mut display = initialised_screen();
        send_command(&mut display, MADCTL);
        send_data(&mut display, &[0x40]);
        set_window(&mut display, 0, 0, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00]);
        assert_eq!(display.shown_pixel(239, 0), RED);
        assert_eq!(display.shown_pixel(0, 0), BLACK);

        send_command(&mut display, MADCTL);
        send_data(&mut display, &[0x20]);
        set_window(&mut display, 5, 5, 0, 0);
        write_pixels(&mut display, &[0x07, 0xE0]);
        assert_eq!(display.shown_pixel(0, 5), GREEN);

        send_command(&mut display, MADCTL);
        send_data(&mut display, &[0x60]);
        set_window(&mut display, 7, 7, 0, 0);
        write_pixels(&mut display, &[0x00, 0x1F]);
        assert_eq!(display.shown_pixel(239, 7), BLUE);

        send_command(&mut display, MADCTL);
        send_data(&mut display, &[0x80]);
        set_window(&mut display, 0, 0, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00]);
        assert_eq!(display.memory_pixel(0, 319), 0xF800);
    }

    #[test]
    fn the_colour_order_bit_swaps_red_and_blue() {
        let mut display = initialised_screen();
        send_command(&mut display, MADCTL);
        send_data(&mut display, &[MADCTL_BLUE_GREEN_RED]);
        set_window(&mut display, 0, 0, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00]);

        assert_eq!(display.shown_pixel(0, 0), BLUE);
        assert_eq!(display.memory_access_control(), MADCTL_BLUE_GREEN_RED);
    }

    #[test]
    fn continues_a_memory_write_across_transmissions() {
        let mut display = initialised_screen();
        set_window(&mut display, 0, 2, 0, 0);
        send_command(&mut display, RAMWR);
        send_data(&mut display, &[0xF8]);
        display.finish_transmission();
        send_data(&mut display, &[0x00, 0x07]);
        display.finish_transmission();
        send_data(&mut display, &[0xE0]);

        send_command(&mut display, RAMWRC);
        send_data(&mut display, &[0x00, 0x1F]);

        assert_eq!(display.shown_pixel(0, 0), RED);
        assert_eq!(display.shown_pixel(1, 0), GREEN);
        assert_eq!(display.shown_pixel(2, 0), BLUE);
    }

    #[test]
    fn the_write_pointer_wraps_inside_the_window() {
        let mut display = initialised_screen();
        set_window(&mut display, 0, 1, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00, 0x07, 0xE0, 0x00, 0x1F]);

        assert_eq!(display.shown_pixel(0, 0), BLUE);
        assert_eq!(display.shown_pixel(1, 0), GREEN);
        assert_eq!(display.shown_pixel(0, 1), BLACK);
    }

    #[test]
    fn takes_12_bit_and_18_bit_pixels() {
        let mut display = initialised_screen();
        send_command(&mut display, COLMOD);
        send_data(&mut display, &[0x53]);
        set_window(&mut display, 0, 1, 0, 0);
        write_pixels(&mut display, &[0xF0, 0x00, 0x0F]);
        assert_eq!(display.memory_pixel(0, 0), 0xF800);
        assert_eq!(display.memory_pixel(1, 0), 0x001F);

        send_command(&mut display, COLMOD);
        send_data(&mut display, &[0x66]);
        set_window(&mut display, 0, 0, 1, 1);
        write_pixels(&mut display, &[0x00, 0xFC, 0x00]);
        assert_eq!(display.memory_pixel(0, 1), 0x07E0);
        assert_eq!(display.pixel_format_setting(), 0x66);
    }

    #[test]
    fn shows_black_while_asleep_or_off() {
        let mut display = initialised_screen();
        set_window(&mut display, 0, 0, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00]);

        send_command(&mut display, SLPIN);
        assert!(display.sleeping());
        assert_eq!(display.shown_pixel(0, 0), BLACK);

        send_command(&mut display, SLPOUT);
        assert_eq!(display.shown_pixel(0, 0), RED);

        send_command(&mut display, DISPOFF);
        assert!(!display.display_on());
        assert_eq!(display.shown_pixel(0, 0), BLACK);
    }

    #[test]
    fn reset_pin_clears_the_controller_state() {
        let mut display = initialised_screen();
        set_window(&mut display, 0, 0, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00]);

        display.set_reset_pin(false);
        assert!(display.in_reset());
        assert_eq!(display.shown_pixel(0, 0), BLACK);
        assert_eq!(display.transfer(0x29), 0);
        display.set_reset_pin(true);

        assert!(!display.in_reset());
        assert!(display.sleeping());
        assert!(!display.display_on());
        assert!(!display.inverted());
        assert_eq!(display.pixel_format_setting(), 0x66);
        assert_eq!(display.memory_pixel(0, 0), 0xF800);
    }

    #[test]
    fn dims_with_the_backlight_pins() {
        let mut display = initialised_screen();
        set_window(&mut display, 0, 0, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00]);

        display.set_backlight_line(BacklightLine::Low, false);
        assert_eq!(display.backlight_level(), 1);
        assert_eq!(display.shown_pixel(0, 0), 0x660000);

        display.set_backlight_line(BacklightLine::High, false);
        assert_eq!(display.shown_pixel(0, 0), RED);

        display.set_backlight_line(BacklightLine::Low, true);
        display.set_backlight_line(BacklightLine::High, true);
        assert_eq!(display.shown_pixel(0, 0), BLACK);
    }

    #[test]
    fn answers_identification_and_power_reads() {
        let mut display = initialised_screen();
        send_command(&mut display, RDDID);
        display.set_data_command_pin(true);
        display.transfer(0x00);
        assert_eq!(display.transfer(0x00), 0x85);
        assert_eq!(display.transfer(0x00), 0x85);
        assert_eq!(display.transfer(0x00), 0x52);

        send_command(&mut display, RDDPM);
        display.set_data_command_pin(true);
        assert_eq!(display.transfer(0x00), 0x9C);

        send_command(&mut display, RDID3);
        display.set_data_command_pin(true);
        assert_eq!(display.transfer(0x00), MODULE_ID);
    }

    #[test]
    fn saves_a_png_screenshot_and_a_stable_image() {
        let mut display = initialised_screen();
        set_window(&mut display, 0, 1, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00, 0x07, 0xE0]);
        let path = std::env::temp_dir().join(format!("st7789-screenshot-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&path);

        display.save_screenshot(&path).unwrap();

        let file = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&file[..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        assert!(file.len() > 100);

        let first = display.visible_frame_rgb888_vec();
        let second = display.visible_frame_rgb888_vec();
        assert_eq!(first, second);
        assert_eq!(&first[..6], &[0xFF, 0, 0, 0, 0xFF, 0]);
        send_command(&mut display, DISPOFF);
        assert_ne!(first, display.visible_frame_rgb888_vec());
    }

    #[test]
    fn the_rgb565_frame_matches_the_shown_colours_before_dimming() {
        let mut display = initialised_screen();
        set_window(&mut display, 3, 3, 4, 4);
        write_pixels(&mut display, &[0x07, 0xE0]);
        let mut frame = vec![0u16; SCREEN_WIDTH * SCREEN_HEIGHT];

        display.visible_frame_rgb565(&mut frame);

        assert_eq!(frame[4 * SCREEN_WIDTH + 3], 0x07E0);
        assert_eq!(frame[0], 0);
        send_command(&mut display, SLPIN);
        frame[0] = 0x1234;
        display.visible_frame_rgb565(&mut frame);
        assert!(frame.iter().all(|pixel| *pixel == 0));
    }

    #[test]
    fn the_frame_generation_changes_with_every_visible_change() {
        let mut display = initialised_screen();
        let before = display.frame_generation();
        set_window(&mut display, 0, 0, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00]);
        let after_pixel = display.frame_generation();
        assert_ne!(before, after_pixel);

        display.set_backlight_line(BacklightLine::Mid, false);
        assert_ne!(after_pixel, display.frame_generation());
    }

    #[test]
    fn status_flags_match_the_window_frame_file() {
        let mut display = St7789::new();
        send_command(&mut display, SLPOUT);
        send_command(&mut display, COLMOD);
        send_data(&mut display, &[0x55]);
        send_command(&mut display, INVON);
        send_command(&mut display, DISPON);
        set_window(&mut display, 1, 1, 0, 0);
        write_pixels(&mut display, &[0xF8, 0x00]);

        assert_eq!(display.status_flags(), FLAG_DISPLAY_ON | FLAG_INVERTED);
        assert_eq!(display.backlight_level(), 3);
        assert_eq!(&display.visible_frame_rgb888_vec()[3..6], &[0xFF, 0x00, 0x00]);
    }

    #[test]
    fn display_status_read_reports_the_modes() {
        let mut display = initialised_screen();
        send_command(&mut display, RDDST);
        display.set_data_command_pin(true);
        let bytes: Vec<u8> = (0..5).map(|_| display.transfer(0)).collect();

        assert_eq!(bytes, vec![0x00, 0x80, 0x53, 0x24, 0x00]);
    }
}
