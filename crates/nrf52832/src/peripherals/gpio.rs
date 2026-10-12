// SPDX-License-Identifier: GPL-3.0-only
//! GPIO port P0: each pin has a real level built from the chip's driver, outside drivers, internal
//! pulls and board pulls, with SENSE, LATCH and the DETECT signal.

// ============================================================================
// Registers
// ============================================================================

const OUT: u32 = 0x504;
const OUTSET: u32 = 0x508;
const OUTCLR: u32 = 0x50C;
const IN: u32 = 0x510;
const DIR: u32 = 0x514;
const DIRSET: u32 = 0x518;
const DIRCLR: u32 = 0x51C;
const LATCH: u32 = 0x520;
const DETECTMODE: u32 = 0x524;
const PIN_CNF: u32 = 0x700;
const PIN_CNF_END: u32 = 0x780;

const PIN_COUNT: usize = 32;
const PIN_CNF_RESET: u32 = 0x2;
const PIN_CNF_MASK: u32 = 0x0003_070F;

const PULL_DOWN: u32 = 1;
const PULL_UP: u32 = 3;
const SENSE_HIGH: u32 = 2;
const SENSE_LOW: u32 = 3;

// Drive modes that let go of the line for a one, and for a zero.
const DRIVE_S0D1: u32 = 6;
const DRIVE_H0D1: u32 = 7;
const DRIVE_D0S1: u32 = 4;
const DRIVE_D0H1: u32 = 5;

/// Who controls a pin besides the GPIO registers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpioteControl {
    None,
    Input,
    Output(bool),
}

// ============================================================================
// Pins
// ============================================================================

#[derive(Clone, Copy)]
struct Pin {
    configuration: u32,
    external: Option<bool>,
    board_pull: Option<bool>,
    gpiote: GpioteControl,
    latched: bool,
    level: bool,
}

impl Pin {
    const fn new() -> Pin {
        Pin {
            configuration: PIN_CNF_RESET,
            external: None,
            board_pull: None,
            gpiote: GpioteControl::None,
            latched: false,
            level: false,
        }
    }

    const fn is_output(&self) -> bool {
        self.configuration & 1 != 0
    }

    const fn input_disconnected(&self) -> bool {
        self.configuration & 2 != 0
    }

    const fn pull(&self) -> u32 {
        (self.configuration >> 2) & 3
    }

    const fn drive(&self) -> u32 {
        (self.configuration >> 8) & 7
    }

    const fn sense(&self) -> u32 {
        (self.configuration >> 16) & 3
    }

    /// The level the chip's own driver puts on the pin, if it drives it; open-drain modes let go of one level.
    fn chip_drive(&self, output_latch: bool) -> Option<bool> {
        let level = match self.gpiote {
            GpioteControl::Output(level) => level,
            GpioteControl::None if self.is_output() => output_latch,
            _ => return None,
        };
        let released = if level {
            matches!(self.drive(), DRIVE_S0D1 | DRIVE_H0D1)
        } else {
            matches!(self.drive(), DRIVE_D0S1 | DRIVE_D0H1)
        };
        (!released).then_some(level)
    }

    fn compute_level(&self, output_latch: bool) -> bool {
        if let Some(level) = self.chip_drive(output_latch) {
            return level;
        }
        if let Some(level) = self.external {
            return level;
        }
        match self.pull() {
            PULL_UP => true,
            PULL_DOWN => false,
            _ => self.board_pull.unwrap_or(false),
        }
    }

    // Sensing looks through the input buffer, so a pin with a disconnected buffer never meets its condition.
    fn sense_condition_met(&self) -> bool {
        if self.input_disconnected() {
            return false;
        }
        match self.sense() {
            SENSE_HIGH => self.level,
            SENSE_LOW => !self.level,
            _ => false,
        }
    }
}

// ============================================================================
// Port
// ============================================================================

pub(crate) struct Gpio {
    pins: [Pin; PIN_COUNT],
    output_latch: u32,
    latched_detect_mode: bool,
    detect_signal: bool,
    changed_levels: u32,
    detect_rises: u32,
    detect_rise_count: u64,
}

impl Gpio {
    pub fn new() -> Gpio {
        let mut gpio = Gpio {
            pins: [Pin::new(); PIN_COUNT],
            output_latch: 0,
            latched_detect_mode: false,
            detect_signal: false,
            changed_levels: 0,
            detect_rises: 0,
            detect_rise_count: 0,
        };
        gpio.reset();
        gpio
    }

    /// Returns every pin to a disconnected input; outside drivers and board pulls stay.
    pub fn reset(&mut self) {
        for pin in &mut self.pins {
            pin.configuration = PIN_CNF_RESET;
            pin.gpiote = GpioteControl::None;
            pin.latched = false;
        }
        self.output_latch = 0;
        self.latched_detect_mode = false;
        self.detect_signal = false;
        self.update_all_pins();
    }

    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            OUT | OUTSET | OUTCLR => self.output_latch,
            IN => self.collect(|pin| !pin.input_disconnected() && pin.level),
            DIR | DIRSET | DIRCLR => self.collect(Pin::is_output),
            LATCH => self.collect(|pin| pin.latched),
            DETECTMODE => self.latched_detect_mode as u32,
            PIN_CNF..PIN_CNF_END => self.pins[((offset - PIN_CNF) / 4) as usize].configuration,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        match offset {
            OUT => self.output_latch = value,
            OUTSET => self.output_latch |= value,
            OUTCLR => self.output_latch &= !value,
            DIR => self.set_directions(value),
            DIRSET => self.set_directions(self.collect(Pin::is_output) | value),
            DIRCLR => self.set_directions(self.collect(Pin::is_output) & !value),
            LATCH => self.clear_latches(value),
            DETECTMODE => self.latched_detect_mode = value & 1 != 0,
            PIN_CNF..PIN_CNF_END => {
                self.pins[((offset - PIN_CNF) / 4) as usize].configuration = value & PIN_CNF_MASK;
            }
            _ => return,
        }
        self.update_all_pins();
    }

    fn collect(&self, bit_is_set: impl Fn(&Pin) -> bool) -> u32 {
        self.pins
            .iter()
            .enumerate()
            .filter(|(_, pin)| bit_is_set(pin))
            .fold(0, |bits, (index, _)| bits | 1 << index)
    }

    fn set_directions(&mut self, outputs: u32) {
        for (index, pin) in self.pins.iter_mut().enumerate() {
            pin.configuration = (pin.configuration & !1) | (outputs >> index) & 1;
        }
    }

    // ========================================================================
    // Control from outside and from GPIOTE
    // ========================================================================

    /// An outside part drives the pin, or lets it go with `None`.
    pub fn drive_externally(&mut self, pin: u8, level: Option<bool>) {
        if let Some(state) = self.pins.get_mut(pin as usize) {
            state.external = level;
            self.update_all_pins();
        }
    }

    /// Fits a board resistor to VCC (`Some(true)`), to ground (`Some(false)`), or removes it.
    pub fn set_board_pull(&mut self, pin: u8, pull: Option<bool>) {
        if let Some(state) = self.pins.get_mut(pin as usize) {
            state.board_pull = pull;
            self.update_all_pins();
        }
    }

    pub fn set_gpiote_control(&mut self, pin: u8, control: GpioteControl) {
        if let Some(state) = self.pins.get_mut(pin as usize) {
            state.gpiote = control;
            self.update_all_pins();
        }
    }

    pub fn gpiote_output_level(&self, pin: u8) -> bool {
        matches!(
            self.pins.get(pin as usize).map(|state| state.gpiote),
            Some(GpioteControl::Output(true))
        )
    }

    // ========================================================================
    // Levels
    // ========================================================================

    pub fn level(&self, pin: u8) -> bool {
        self.pins.get(pin as usize).is_some_and(|state| state.level)
    }

    /// The electrical level of every pin, one bit each; a floating pin reads low.
    pub fn levels(&self) -> u32 {
        self.collect(|pin| pin.level)
    }

    /// The pins whose level the chip's own driver sets.
    pub fn driven_pins(&self) -> u32 {
        let latch = self.output_latch;
        self.pins
            .iter()
            .enumerate()
            .filter(|(index, pin)| pin.chip_drive(latch >> index & 1 != 0).is_some())
            .fold(0, |bits, (index, _)| bits | 1 << index)
    }

    fn update_all_pins(&mut self) {
        for index in 0..PIN_COUNT {
            let pin = &mut self.pins[index];
            let level = pin.compute_level(self.output_latch >> index & 1 != 0);
            if level != pin.level {
                pin.level = level;
                self.changed_levels |= 1 << index;
            }
        }
        self.update_detect();
    }

    /// The pins whose level changed since the last call.
    pub fn take_changed_levels(&mut self) -> u32 {
        std::mem::take(&mut self.changed_levels)
    }

    // ========================================================================
    // SENSE, LATCH and DETECT
    // ========================================================================

    fn update_detect(&mut self) {
        let mut any_pin_detects = false;
        let mut any_pin_latched = false;
        for pin in &mut self.pins {
            if pin.sense_condition_met() {
                any_pin_detects = true;
                pin.latched = true;
            }
            any_pin_latched |= pin.latched;
        }
        self.set_detect_signal(if self.latched_detect_mode {
            any_pin_latched
        } else {
            any_pin_detects
        });
    }

    fn clear_latches(&mut self, bits: u32) {
        for (index, pin) in self.pins.iter_mut().enumerate() {
            if bits >> index & 1 != 0 {
                pin.latched = false;
            }
        }
        // A clear that leaves latched pins behind gives LDETECT a fresh rising edge.
        if self.latched_detect_mode {
            self.detect_signal = false;
        }
    }

    fn set_detect_signal(&mut self, value: bool) {
        if value != self.detect_signal {
            self.detect_signal = value;
            if value {
                self.detect_rises += 1;
                self.detect_rise_count += 1;
            }
        }
    }

    /// How many times DETECT rose since the last call.
    pub fn take_detect_rises(&mut self) -> u32 {
        std::mem::take(&mut self.detect_rises)
    }

    pub fn detect_rise_count(&self) -> u64 {
        self.detect_rise_count
    }
}
