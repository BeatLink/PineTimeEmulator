// SPDX-License-Identifier: GPL-3.0-only
//! Every peripheral, found by its id (bits 12..17 of its address), and the work that follows an
//! access: PPI tasks, pin changes, the board's view of the pins, and the radio's due work.

use std::collections::HashMap;

use crate::chip::{ResetKind, SerialPort};
use crate::context::Context;
use crate::peripherals::ccm_aar::CcmAar;
use crate::peripherals::comp_lpcomp::CompLpcomp;
use crate::peripherals::ecb::Ecb;
use crate::peripherals::egu::Egu;
use crate::peripherals::gpio::Gpio;
use crate::peripherals::gpiote::Gpiote;
use crate::peripherals::mwu::Mwu;
use crate::peripherals::nvmc::Nvmc;
use crate::peripherals::power_clock::PowerClock;
use crate::peripherals::pwm::Pwm;
use crate::peripherals::qdec::Qdec;
use crate::peripherals::radio::{self, Radio};
use crate::peripherals::rng::Rng;
use crate::peripherals::rtc::Rtc;
use crate::peripherals::saadc::Saadc;
use crate::peripherals::serial::SerialInterface;
use crate::peripherals::temp::Temp;
use crate::peripherals::timer::Timer;
use crate::peripherals::uart::{Uart, UART0_BASE, UARTE1_BASE};
use crate::peripherals::watchdog::Watchdog;
use crate::ppi::Ppi;
use crate::variant::Variant;

// ============================================================================
// Peripheral ids
// ============================================================================

pub(crate) const PERIPHERAL_START: u32 = 0x4000_0000;
pub(crate) const GPIO_START: u32 = 0x5000_0000;
pub(crate) const GPIO_END: u32 = 0x5000_1000;
/// The nRF52840's port P1 sits 0x300 above P0, so its registers start at 0x50000800.
const GPIO_PORT1_OFFSET: u32 = 0x300;
const GPIO_PORT1_REGISTERS_START: u32 = GPIO_START + 0x800;

pub(crate) const POWER_CLOCK: u8 = 0x00;
pub(crate) const RADIO: u8 = 0x01;
pub(crate) const UART: u8 = 0x02;
pub(crate) const SERIAL0: u8 = 0x03;
pub(crate) const SERIAL1: u8 = 0x04;
pub(crate) const GPIOTE: u8 = 0x06;
pub(crate) const SAADC: u8 = 0x07;
pub(crate) const TIMER0: u8 = 0x08;
pub(crate) const TIMER1: u8 = 0x09;
pub(crate) const TIMER2: u8 = 0x0A;
pub(crate) const RTC0: u8 = 0x0B;
pub(crate) const TEMP: u8 = 0x0C;
pub(crate) const RNG: u8 = 0x0D;
pub(crate) const ECB: u8 = 0x0E;
pub(crate) const CCM_AAR: u8 = 0x0F;
pub(crate) const WATCHDOG: u8 = 0x10;
pub(crate) const RTC1: u8 = 0x11;
pub(crate) const QDEC: u8 = 0x12;
pub(crate) const COMP_LPCOMP: u8 = 0x13;
pub(crate) const EGU0: u8 = 0x14;
pub(crate) const EGU5: u8 = 0x19;
pub(crate) const TIMER3: u8 = 0x1A;
pub(crate) const TIMER4: u8 = 0x1B;
pub(crate) const PWM0: u8 = 0x1C;
pub(crate) const NVMC: u8 = 0x1E;
pub(crate) const PPI: u8 = 0x1F;
pub(crate) const MWU: u8 = 0x20;
pub(crate) const PWM1: u8 = 0x21;
pub(crate) const PWM2: u8 = 0x22;
pub(crate) const SERIAL2: u8 = 0x23;
pub(crate) const RTC2: u8 = 0x24;
pub(crate) const UARTE1: u8 = 0x28;

const fn base_of(id: u8) -> u32 {
    PERIPHERAL_START + ((id as u32) << 12)
}

/// A chain of PPI tasks and events longer than this is cut off, as it would hang the emulator.
const MAXIMUM_SETTLE_STEPS: u32 = 100_000;

// ============================================================================
// Peripherals
// ============================================================================

pub(crate) struct Peripherals {
    nrf52840: bool,
    pub power_clock: PowerClock,
    pub radio: Radio,
    pub uart: Uart,
    pub serial: [SerialInterface; 3],
    pub gpio: Gpio,
    pub gpiote: Gpiote,
    pub saadc: Saadc,
    pub timers: [Timer; 5],
    pub rtcs: [Rtc; 3],
    pub temp: Temp,
    pub rng: Rng,
    pub ecb: Ecb,
    pub ccm_aar: CcmAar,
    pub watchdog: Watchdog,
    pub qdec: Qdec,
    pub comp_lpcomp: CompLpcomp,
    pub egus: [Egu; 6],
    pub pwms: [Pwm; 3],
    pub nvmc: Nvmc,
    pub mwu: Mwu,
    pub ppi: Ppi,
    /// The nRF52840's UARTE1 and GPIO port P1, used only on that variant.
    pub uarte1: Uart,
    pub gpio_port1: Gpio,
    unknown_registers: HashMap<u32, u32>,
    pub unhandled_accesses: u64,
    pub cut_off_chains: u64,
    announced_pins: Option<(u32, u32)>,
    tasks: Vec<u32>,
}

impl Peripherals {
    pub fn new(variant: Variant) -> Peripherals {
        let nrf52840 = variant.is_nrf52840();
        Peripherals {
            nrf52840,
            power_clock: PowerClock::new(),
            radio: Radio::new(nrf52840),
            uart: Uart::new(UART0_BASE, nrf52840),
            serial: [
                SerialInterface::new(base_of(SERIAL0), 0),
                SerialInterface::new(base_of(SERIAL1), 1),
                SerialInterface::new(base_of(SERIAL2), 2),
            ],
            gpio: Gpio::new(),
            gpiote: Gpiote::new(),
            saadc: Saadc::new(),
            timers: [
                Timer::new(base_of(TIMER0), 4),
                Timer::new(base_of(TIMER1), 4),
                Timer::new(base_of(TIMER2), 4),
                Timer::new(base_of(TIMER3), 6),
                Timer::new(base_of(TIMER4), 6),
            ],
            rtcs: [
                Rtc::new(base_of(RTC0), 3),
                Rtc::new(base_of(RTC1), 4),
                Rtc::new(base_of(RTC2), 4),
            ],
            temp: Temp::new(),
            rng: Rng::new(),
            ecb: Ecb::new(),
            ccm_aar: CcmAar::new(),
            watchdog: Watchdog::new(),
            qdec: Qdec::new(),
            comp_lpcomp: CompLpcomp::new(),
            egus: core::array::from_fn(|index| Egu::new(base_of(EGU0 + index as u8))),
            pwms: [
                Pwm::new(base_of(PWM0)),
                Pwm::new(base_of(PWM1)),
                Pwm::new(base_of(PWM2)),
            ],
            nvmc: Nvmc::new(),
            mwu: Mwu::new(),
            ppi: Ppi::new(),
            uarte1: Uart::new(UARTE1_BASE, true),
            gpio_port1: Gpio::new(),
            unknown_registers: HashMap::new(),
            unhandled_accesses: 0,
            cut_off_chains: 0,
            announced_pins: None,
            tasks: Vec::new(),
        }
    }

    // ========================================================================
    // Reset
    // ========================================================================

    /// Resets every peripheral; the watchdog survives a soft reset and a lockup, as on the chip.
    pub fn reset(&mut self, kind: ResetKind, context: &mut Context) {
        let watchdog_survives = matches!(kind, ResetKind::SoftReset | ResetKind::Lockup);
        if !watchdog_survives {
            self.watchdog.reset(context);
        }
        self.power_clock.set_watchdog_running(self.watchdog.running());
        self.power_clock.reset(context);
        self.radio.reset(context);
        self.uart.reset();
        self.uarte1.reset();
        self.gpio_port1.reset();
        for serial in &mut self.serial {
            serial.reset(context);
        }
        self.gpiote.reset(&mut self.gpio);
        self.gpio.reset();
        self.saadc.reset(context);
        for timer in &mut self.timers {
            timer.reset(context);
        }
        for rtc in &mut self.rtcs {
            rtc.reset(context);
        }
        self.temp.reset();
        self.rng.reset(context);
        self.ecb.reset();
        self.ccm_aar.reset();
        self.qdec.reset();
        self.comp_lpcomp.reset();
        for egu in &mut self.egus {
            egu.reset();
        }
        for pwm in &mut self.pwms {
            pwm.reset(context);
        }
        self.nvmc.reset();
        self.mwu.reset();
        self.ppi.reset();
        self.unknown_registers.clear();
        // Parts wired to pins learn the level of each pin again after every reset.
        self.announced_pins = None;
        self.watchdog.reschedule(context);
    }

    // ========================================================================
    // Registers
    // ========================================================================

    /// Reads a word register at `address`, which must lie in the peripheral or GPIO space.
    pub fn read(&mut self, address: u32, context: &mut Context) -> u32 {
        let offset = address & 0xFFC;
        if (GPIO_START..GPIO_END).contains(&address) {
            if self.nrf52840 && address >= GPIO_PORT1_REGISTERS_START {
                return self.gpio_port1.read((address - GPIO_PORT1_OFFSET) & 0xFFC);
            }
            return self.gpio.read(offset);
        }
        let id = ((address - PERIPHERAL_START) >> 12) as u8;
        if address >= base_of(RTC2) + 0x1000 {
            if self.nrf52840 && id == UARTE1 {
                context.touch(id);
                return self.uarte1.read(offset, context);
            }
            return self.read_unknown(address);
        }
        context.touch(id);
        match id {
            POWER_CLOCK => self.power_clock.read(offset),
            RADIO => {
                self.settle(context);
                self.radio.read(offset)
            }
            UART => self.uart.read(offset, context),
            SERIAL0 | SERIAL1 => self.serial[(id - SERIAL0) as usize].read(offset, context),
            SERIAL2 => self.serial[2].read(offset, context),
            GPIOTE => self.gpiote.read(offset),
            SAADC => self.saadc.read(offset),
            TIMER0..=TIMER2 => self.timers[(id - TIMER0) as usize].read(offset, context),
            TIMER3 | TIMER4 => self.timers[(id - TIMER3 + 3) as usize].read(offset, context),
            RTC0 => self.rtcs[0].read(offset, context),
            RTC1 => self.rtcs[1].read(offset, context),
            RTC2 => self.rtcs[2].read(offset, context),
            TEMP => self.temp.read(offset),
            RNG => self.rng.read(offset),
            ECB => self.ecb.read(offset),
            CCM_AAR => self.ccm_aar.read(offset),
            WATCHDOG => self.watchdog.read(offset),
            QDEC => self.qdec.read(offset),
            COMP_LPCOMP => self.comp_lpcomp.read(offset),
            EGU0..=EGU5 => self.egus[(id - EGU0) as usize].read(offset),
            PWM0 => self.pwms[0].read(offset),
            PWM1 => self.pwms[1].read(offset),
            PWM2 => self.pwms[2].read(offset),
            NVMC => self.nvmc.read(offset),
            PPI => self.ppi.read(offset),
            MWU => self.mwu.read(offset),
            _ => self.read_unknown(address),
        }
    }

    /// Writes a word register at `address`, which must lie in the peripheral or GPIO space.
    pub fn write(&mut self, address: u32, value: u32, context: &mut Context) {
        let offset = address & 0xFFC;
        if (GPIO_START..GPIO_END).contains(&address) {
            if self.nrf52840 && address >= GPIO_PORT1_REGISTERS_START {
                self.gpio_port1
                    .write((address - GPIO_PORT1_OFFSET) & 0xFFC, value);
                return;
            }
            self.gpio.write(offset, value);
            return;
        }
        let id = ((address - PERIPHERAL_START) >> 12) as u8;
        if address >= base_of(RTC2) + 0x1000 {
            if self.nrf52840 && id == UARTE1 {
                context.touch(id);
                self.uarte1.write(offset, value, context);
                return;
            }
            self.write_unknown(address, value);
            return;
        }
        context.touch(id);
        match id {
            POWER_CLOCK => self.power_clock.write(offset, value, context),
            RADIO => {
                self.settle(context);
                self.radio.write(offset, value, context);
            }
            UART => self.uart.write(offset, value, context),
            SERIAL0 | SERIAL1 => self.serial[(id - SERIAL0) as usize].write(offset, value, context),
            SERIAL2 => self.serial[2].write(offset, value, context),
            GPIOTE => self.gpiote.write(offset, value, &mut self.gpio),
            SAADC => self.saadc.write(offset, value, context),
            TIMER0..=TIMER2 => self.timers[(id - TIMER0) as usize].write(offset, value, context),
            TIMER3 | TIMER4 => self.timers[(id - TIMER3 + 3) as usize].write(offset, value, context),
            RTC0 => self.rtcs[0].write(offset, value, context),
            RTC1 => self.rtcs[1].write(offset, value, context),
            RTC2 => self.rtcs[2].write(offset, value, context),
            TEMP => self.temp.write(offset, value, context),
            RNG => self.rng.write(offset, value, context),
            ECB => self.ecb.write(offset, value, context),
            CCM_AAR => self.ccm_aar.write(offset, value, context),
            WATCHDOG => self.watchdog.write(offset, value, context),
            QDEC => self.qdec.write(offset, value, context),
            COMP_LPCOMP => self.comp_lpcomp.write(offset, value, context),
            EGU0..=EGU5 => self.egus[(id - EGU0) as usize].write(offset, value, context),
            PWM0 => self.pwms[0].write(offset, value, context),
            PWM1 => self.pwms[1].write(offset, value, context),
            PWM2 => self.pwms[2].write(offset, value, context),
            NVMC => self.nvmc.write(offset, value, context),
            PPI => self.ppi.write(offset, value),
            MWU => self.mwu.write(offset, value),
            _ => self.write_unknown(address, value),
        }
    }

    /// The UART a host link reaches as `port`, if this variant has it.
    pub fn serial_port_mut(&mut self, port: SerialPort) -> Option<&mut Uart> {
        match port {
            SerialPort::Uart0 => Some(&mut self.uart),
            SerialPort::Uarte1 if self.nrf52840 => Some(&mut self.uarte1),
            SerialPort::Uarte1 => None,
        }
    }

    // Undocumented registers, such as the ones errata workarounds poke, keep what is written.
    fn read_unknown(&mut self, address: u32) -> u32 {
        self.unhandled_accesses += 1;
        self.unknown_registers.get(&(address & !3)).copied().unwrap_or(0)
    }

    fn write_unknown(&mut self, address: u32, value: u32) {
        self.unhandled_accesses += 1;
        self.unknown_registers.insert(address & !3, value);
    }

    // ========================================================================
    // Events
    // ========================================================================

    pub fn handle_event(&mut self, id: u8, token: u32, context: &mut Context) {
        context.touch(id);
        match id {
            POWER_CLOCK => self.power_clock.handle_event(token, context),
            RADIO => self.radio.alarm_fired(),
            SERIAL0 | SERIAL1 => self.serial[(id - SERIAL0) as usize].handle_event(token, context),
            SERIAL2 => self.serial[2].handle_event(token, context),
            SAADC => self.saadc.handle_event(token, context),
            TIMER0..=TIMER2 => self.timers[(id - TIMER0) as usize].handle_event(token, context),
            TIMER3 | TIMER4 => self.timers[(id - TIMER3 + 3) as usize].handle_event(token, context),
            RTC0 => self.rtcs[0].handle_event(token, context),
            RTC1 => self.rtcs[1].handle_event(token, context),
            RTC2 => self.rtcs[2].handle_event(token, context),
            RNG => self.rng.handle_event(token, context),
            WATCHDOG => self.watchdog.handle_event(token, context),
            PWM0 => self.pwms[0].handle_event(token, context),
            PWM1 => self.pwms[1].handle_event(token, context),
            PWM2 => self.pwms[2].handle_event(token, context),
            _ => {}
        }
    }

    /// Whether the peripheral with `id` holds its interrupt line high.
    pub fn interrupt_pending(&self, id: u8) -> bool {
        match id {
            POWER_CLOCK => self.power_clock.interrupt_pending(),
            RADIO => self.radio.interrupt_pending(),
            UART => self.uart.interrupt_pending(),
            SERIAL0 | SERIAL1 => self.serial[(id - SERIAL0) as usize].interrupt_pending(),
            SERIAL2 => self.serial[2].interrupt_pending(),
            GPIOTE => self.gpiote.interrupt_pending(),
            SAADC => self.saadc.interrupt_pending(),
            TIMER0..=TIMER2 => self.timers[(id - TIMER0) as usize].interrupt_pending(),
            TIMER3 | TIMER4 => self.timers[(id - TIMER3 + 3) as usize].interrupt_pending(),
            RTC0 => self.rtcs[0].interrupt_pending(),
            RTC1 => self.rtcs[1].interrupt_pending(),
            RTC2 => self.rtcs[2].interrupt_pending(),
            TEMP => self.temp.interrupt_pending(),
            RNG => self.rng.interrupt_pending(),
            ECB => self.ecb.interrupt_pending(),
            CCM_AAR => self.ccm_aar.interrupt_pending(),
            WATCHDOG => self.watchdog.interrupt_pending(),
            QDEC => self.qdec.interrupt_pending(),
            COMP_LPCOMP => self.comp_lpcomp.interrupt_pending(),
            EGU0..=EGU5 => self.egus[(id - EGU0) as usize].interrupt_pending(),
            PWM0 => self.pwms[0].interrupt_pending(),
            PWM1 => self.pwms[1].interrupt_pending(),
            PWM2 => self.pwms[2].interrupt_pending(),
            MWU => self.mwu.interrupt_pending(),
            UARTE1 => self.uarte1.interrupt_pending(),
            _ => false,
        }
    }

    // ========================================================================
    // Settling
    // ========================================================================

    /// Runs everything an access set off, at once: PPI tasks of raised events, pin changes the
    /// board asked for, GPIOTE edges and DETECT, the board's view of the pins, and due radio work.
    pub fn settle(&mut self, context: &mut Context) {
        for _ in 0..MAXIMUM_SETTLE_STEPS {
            if let Some(event) = context.signals.raised_events.pop_front() {
                self.run_tasks_of(event, context);
                continue;
            }
            if !context.signals.pin_changes.is_empty() {
                for change in std::mem::take(&mut context.signals.pin_changes) {
                    self.gpio.drive_externally(change.pin, change.level);
                }
                continue;
            }
            if self.follow_pin_activity(context) {
                continue;
            }
            if self.radio.has_due_work(context.now) {
                self.radio.run_one_due_work(context);
                context.touch(radio::ID);
                continue;
            }
            self.finish_settling(context);
            return;
        }
        self.cut_off_chains += 1;
        context.signals.raised_events.clear();
        self.finish_settling(context);
    }

    fn finish_settling(&mut self, context: &mut Context) {
        self.radio.arm_alarm(context);
        self.power_clock.set_watchdog_running(self.watchdog.running());
    }

    fn run_tasks_of(&mut self, event: u32, context: &mut Context) {
        if !self.ppi.has_enabled_channels() {
            return;
        }
        let mut tasks = std::mem::take(&mut self.tasks);
        self.ppi.tasks_for_event(event, &mut tasks);
        for task in tasks.drain(..) {
            if (PERIPHERAL_START..GPIO_END).contains(&task) {
                self.write(task, 1, context);
            }
        }
        self.tasks = tasks;
    }

    /// Passes pin level changes on to GPIOTE, POWER and the board; says whether anything changed.
    fn follow_pin_activity(&mut self, context: &mut Context) -> bool {
        let changed = self.gpio.take_changed_levels();
        let rises = self.gpio.take_detect_rises();
        if changed != 0 {
            self.gpiote.pins_changed(changed, &self.gpio, context);
            context.touch(GPIOTE);
        }
        for _ in 0..rises {
            self.gpiote.detect_rose(context);
            // DETECT wakes the chip from System OFF with a reset.
            if self.power_clock.system_off() {
                context.request_reset(ResetKind::WakeFromSystemOff);
            }
        }
        let pins = (self.gpio.levels(), self.gpio.driven_pins());
        if self.announced_pins != Some(pins) {
            self.announced_pins = Some(pins);
            context.pin_outputs_changed(pins.0, pins.1);
            return true;
        }
        changed != 0 || rises != 0
    }
}
