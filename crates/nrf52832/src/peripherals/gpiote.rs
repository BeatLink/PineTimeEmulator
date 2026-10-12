// SPDX-License-Identifier: GPL-3.0-only
//! GPIO tasks and events: eight channels that watch or drive one pin each, and the PORT event
//! that follows the port's DETECT signal.

use super::gpio::{Gpio, GpioteControl};
use super::{triggers, EventRegisters};
use crate::context::Context;

// ============================================================================
// Registers
// ============================================================================

pub(crate) const BASE: u32 = 0x4000_6000;

const TASKS_OUT: u32 = 0x000;
const TASKS_SET: u32 = 0x030;
const TASKS_CLR: u32 = 0x060;
const TASKS_END: u32 = 0x080;
const CONFIG: u32 = 0x510;
const CONFIG_END: u32 = 0x530;

const CHANNEL_COUNT: usize = 8;
const PORT_EVENT: u32 = 31;
const VALID_EVENTS: u32 = 0x8000_00FF;

const MODE_EVENT: u32 = 1;
const MODE_TASK: u32 = 3;
const POLARITY_LO_TO_HI: u32 = 1;
const POLARITY_HI_TO_LO: u32 = 2;
const POLARITY_TOGGLE: u32 = 3;

#[derive(Clone, Copy, Default)]
struct Channel {
    /// The CONFIG value in force.
    configuration: u32,
    last_level: bool,
}

impl Channel {
    const fn mode(&self) -> u32 {
        self.configuration & 3
    }

    const fn pin(&self) -> u8 {
        ((self.configuration >> 8) & 0x1F) as u8
    }

    const fn polarity(&self) -> u32 {
        (self.configuration >> 16) & 3
    }

    const fn output_initial(&self) -> bool {
        self.configuration & (1 << 20) != 0
    }
}

// ============================================================================
// GPIOTE
// ============================================================================

pub(crate) struct Gpiote {
    channels: [Channel; CHANNEL_COUNT],
    events: EventRegisters,
}

impl Gpiote {
    pub fn new() -> Gpiote {
        Gpiote {
            channels: [Channel::default(); CHANNEL_COUNT],
            events: EventRegisters::new(BASE, VALID_EVENTS),
        }
    }

    pub fn reset(&mut self, gpio: &mut Gpio) {
        for index in 0..CHANNEL_COUNT {
            self.release_pin(index, gpio);
        }
        self.channels = [Channel::default(); CHANNEL_COUNT];
        self.events.reset();
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            CONFIG..CONFIG_END => self.channels[((offset - CONFIG) / 4) as usize].configuration,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, gpio: &mut Gpio) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_OUT..TASKS_END if triggers(value) => {
                let index = ((offset % 0x30) / 4) as usize;
                if index >= CHANNEL_COUNT {
                    return;
                }
                match offset {
                    TASKS_OUT..TASKS_SET => self.run_out_task(index, gpio),
                    TASKS_SET..TASKS_CLR => self.drive_task_pin(index, true, gpio),
                    _ => self.drive_task_pin(index, false, gpio),
                }
            }
            CONFIG..CONFIG_END => self.apply_configuration(((offset - CONFIG) / 4) as usize, value, gpio),
            _ => {}
        }
    }

    // ========================================================================
    // Channels
    // ========================================================================

    // The mode takes effect only after the pin settles, so taking over a pin raises no event.
    fn apply_configuration(&mut self, index: usize, value: u32, gpio: &mut Gpio) {
        self.release_pin(index, gpio);
        let channel = Channel {
            configuration: value & 0x0013_1F03,
            last_level: false,
        };
        let pin = channel.pin();
        let mut applied = channel;
        match channel.mode() {
            MODE_TASK => gpio.set_gpiote_control(pin, GpioteControl::Output(channel.output_initial())),
            MODE_EVENT => {
                gpio.set_gpiote_control(pin, GpioteControl::Input);
                applied.last_level = gpio.level(pin);
            }
            _ => {}
        }
        self.channels[index] = applied;
    }

    // A channel that leaves a pin gives it back to the GPIO registers, unless another channel still claims it.
    fn release_pin(&mut self, index: usize, gpio: &mut Gpio) {
        let channel = self.channels[index];
        if channel.mode() != MODE_EVENT && channel.mode() != MODE_TASK {
            return;
        }
        let pin = channel.pin();
        let level = gpio.gpiote_output_level(pin);
        self.channels[index].configuration &= !3;
        gpio.set_gpiote_control(pin, GpioteControl::None);
        for other in &self.channels {
            if other.pin() != pin {
                continue;
            }
            match other.mode() {
                MODE_TASK => gpio.set_gpiote_control(pin, GpioteControl::Output(level)),
                MODE_EVENT => gpio.set_gpiote_control(pin, GpioteControl::Input),
                _ => {}
            }
        }
    }

    fn run_out_task(&mut self, index: usize, gpio: &mut Gpio) {
        let channel = self.channels[index];
        match channel.polarity() {
            POLARITY_LO_TO_HI => self.drive_task_pin(index, true, gpio),
            POLARITY_HI_TO_LO => self.drive_task_pin(index, false, gpio),
            POLARITY_TOGGLE => self.drive_task_pin(index, !gpio.gpiote_output_level(channel.pin()), gpio),
            _ => {}
        }
    }

    // Tasks on a channel that is not in task mode do nothing.
    fn drive_task_pin(&mut self, index: usize, level: bool, gpio: &mut Gpio) {
        let channel = self.channels[index];
        if channel.mode() == MODE_TASK {
            gpio.set_gpiote_control(channel.pin(), GpioteControl::Output(level));
        }
    }

    // ========================================================================
    // Pin events
    // ========================================================================

    /// Raises IN events on the channels watching pins in `changed` whose edge matches.
    pub fn pins_changed(&mut self, changed: u32, gpio: &Gpio, context: &mut Context) {
        for index in 0..CHANNEL_COUNT {
            let channel = self.channels[index];
            if channel.mode() != MODE_EVENT || changed >> channel.pin() & 1 == 0 {
                continue;
            }
            let level = gpio.level(channel.pin());
            let was_high = channel.last_level;
            self.channels[index].last_level = level;
            let rising = !was_high && level;
            let falling = was_high && !level;
            let fires = match channel.polarity() {
                POLARITY_LO_TO_HI => rising,
                POLARITY_HI_TO_LO => falling,
                POLARITY_TOGGLE => rising || falling,
                _ => false,
            };
            if fires {
                self.events.raise(index as u32, context);
            }
        }
    }

    /// The port's DETECT signal rose.
    pub fn detect_rose(&mut self, context: &mut Context) {
        self.events.raise(PORT_EVENT, context);
    }
}
