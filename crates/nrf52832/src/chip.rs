// SPDX-License-Identifier: GPL-3.0-only
//! The whole chip as the CPU, the machine and the debugger see it.

use pinetime_core::{AccessSize, BoardPorts, BusError, Cycle, EventKey, Owner, PinChange, Scheduler};

use crate::context::{Context, Signals};
use crate::interrupts::InterruptLines;
use crate::memory::{Memory, Region};
use crate::peripheral_bus::{Peripherals, GPIO_END, PERIPHERAL_START};
use crate::peripherals::power_clock::{
    RESET_REASON_LOCKUP, RESET_REASON_PIN, RESET_REASON_SOFT, RESET_REASON_SYSTEM_OFF_GPIO,
    RESET_REASON_WATCHDOG,
};
use crate::peripherals::radio::{RadioFrame, RadioMedium};
use crate::variant::Variant;

// ============================================================================
// Public types
// ============================================================================

/// The board resistors on the PineTime: P0.05 and P0.25 (chip selects), P0.12 and P0.19 (charger lines).
pub const PINETIME_BOARD_PULL_UPS: u32 = 0x0208_1020;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetKind {
    PowerOn,
    Pin,
    Watchdog,
    /// AIRCR.SYSRESETREQ.
    SoftReset,
    Lockup,
    /// A GPIO DETECT signal while in System OFF.
    WakeFromSystemOff,
}

/// The UARTs a host link can attach to; UARTE1 exists only on the nRF52840.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerialPort {
    Uart0,
    Uarte1,
}

/// Which accesses a watchpoint reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchAccess {
    Read,
    Write,
    ReadWrite,
}

/// What the chip reaches outside itself during a call: the time, the scheduler and the board.
pub struct ChipPorts<'a> {
    pub now: Cycle,
    pub scheduler: &'a mut Scheduler,
    pub board: &'a mut dyn BoardPorts,
}

struct Watchpoint {
    address: u32,
    size: u32,
    access: WatchAccess,
}

// ============================================================================
// Chip
// ============================================================================

pub struct Nrf52832 {
    variant: Variant,
    memory: Memory,
    signals: Signals,
    interrupts: InterruptLines,
    peripherals: Peripherals,
    watchpoints: Vec<Watchpoint>,
    watchpoint_hit: Option<(u32, u32, bool)>,
}

impl Default for Nrf52832 {
    fn default() -> Nrf52832 {
        Nrf52832::new()
    }
}

/// Builds the peripheral context from the chip's fields, leaving `peripherals` free to borrow.
macro_rules! context {
    ($chip:expr, $ports:expr) => {
        Context {
            now: $ports.now,
            scheduler: &mut *$ports.scheduler,
            board: &mut *$ports.board,
            memory: &mut $chip.memory,
            signals: &mut $chip.signals,
        }
    };
}

impl Nrf52832 {
    /// A new nRF52832: flash and UICR erased, the FICR filled in, and the PineTime's board pull-ups fitted.
    pub fn new() -> Nrf52832 {
        Nrf52832::new_variant(Variant::Nrf52832)
    }

    /// A new chip of either variant; only the nRF52832 gets the PineTime's board pull-ups.
    pub fn new_variant(variant: Variant) -> Nrf52832 {
        let mut chip = Nrf52832 {
            variant,
            memory: Memory::new(variant),
            signals: Signals::default(),
            interrupts: InterruptLines::default(),
            peripherals: Peripherals::new(variant),
            watchpoints: Vec::new(),
            watchpoint_hit: None,
        };
        if variant == Variant::Nrf52832 {
            for pin in 0..32 {
                if PINETIME_BOARD_PULL_UPS >> pin & 1 != 0 {
                    chip.peripherals.gpio.set_board_pull(pin, Some(true));
                }
            }
        }
        chip
    }

    pub fn variant(&self) -> Variant {
        self.variant
    }

    // ========================================================================
    // CPU accesses
    // ========================================================================

    #[inline]
    pub fn read(&mut self, address: u32, size: AccessSize, ports: &mut ChipPorts) -> Result<u32, BusError> {
        let value = match self.memory.locate(address, size.bytes()) {
            Some((region, offset)) => Memory::read_value(self.memory.bytes(region), offset, size),
            None => self.read_peripheral(address, size, ports)?,
        };
        if !self.watchpoints.is_empty() {
            self.check_watchpoints(address, size, value, false);
        }
        Ok(value)
    }

    #[inline]
    pub fn write(
        &mut self,
        address: u32,
        value: u32,
        size: AccessSize,
        ports: &mut ChipPorts,
    ) -> Result<(), BusError> {
        if !self.watchpoints.is_empty() {
            self.check_watchpoints(address, size, value, true);
        }
        match self.memory.locate(address, size.bytes()) {
            Some((Region::Ram | Region::Errata, offset)) => {
                let region = if address >> 28 == 0xF {
                    Region::Errata
                } else {
                    Region::Ram
                };
                Memory::write_value(self.memory.bytes_mut(region), offset, value, size);
                Ok(())
            }
            // The CPU writes flash and UICR only while NVMC WEN is set, and then can only clear bits, as NOR flash does.
            Some((region @ (Region::Flash | Region::Uicr), offset)) => {
                if self.peripherals.nvmc.writes_enabled() {
                    let bytes = self.memory.bytes_mut(region);
                    let stored = Memory::read_value(bytes, offset, size);
                    Memory::write_value(bytes, offset, stored & value, size);
                } else {
                    self.peripherals.nvmc.count_ignored_write();
                }
                Ok(())
            }
            Some((Region::Ficr, _)) => Ok(()),
            None => self.write_peripheral(address, value, size, ports),
        }
    }

    /// An instruction halfword from flash or RAM, without side effects.
    #[inline]
    pub fn fetch(&self, address: u32) -> Result<u16, BusError> {
        match self.memory.locate(address, 2) {
            Some((region @ (Region::Flash | Region::Ram), offset)) => {
                let bytes = self.memory.bytes(region);
                Ok(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
            }
            _ => Err(BusError::Unmapped),
        }
    }

    fn is_peripheral(address: u32) -> bool {
        (PERIPHERAL_START..GPIO_END).contains(&address)
    }

    // Peripherals take word accesses; a narrower access reads or writes its part of the word.
    fn read_peripheral(
        &mut self,
        address: u32,
        size: AccessSize,
        ports: &mut ChipPorts,
    ) -> Result<u32, BusError> {
        if !Self::is_peripheral(address) {
            return Err(BusError::Unmapped);
        }
        let mut context = context!(self, ports);
        let word = self.peripherals.read(address & !3, &mut context);
        self.peripherals.settle(&mut context);
        self.update_interrupt_lines();
        Ok((word >> (8 * (address & 3))) & size.mask())
    }

    fn write_peripheral(
        &mut self,
        address: u32,
        value: u32,
        size: AccessSize,
        ports: &mut ChipPorts,
    ) -> Result<(), BusError> {
        if !Self::is_peripheral(address) {
            return Err(BusError::Unmapped);
        }
        let mut context = context!(self, ports);
        let word = (value & size.mask()) << (8 * (address & 3));
        self.peripherals.write(address & !3, word, &mut context);
        self.peripherals.settle(&mut context);
        self.update_interrupt_lines();
        Ok(())
    }

    fn update_interrupt_lines(&mut self) {
        let mut touched = std::mem::take(&mut self.signals.touched);
        while touched != 0 {
            let id = touched.trailing_zeros() as u8;
            touched &= touched - 1;
            self.interrupts.set(id, self.peripherals.interrupt_pending(id));
        }
    }

    // ========================================================================
    // Time, interrupts and resets
    // ========================================================================

    /// Handles an event whose owner is `Owner::Peripheral(_)`; other events are ignored.
    pub fn handle_event(&mut self, key: EventKey, ports: &mut ChipPorts) {
        let Owner::Peripheral(id) = key.owner else {
            return;
        };
        let mut context = context!(self, ports);
        self.peripherals.handle_event(id, key.token, &mut context);
        self.peripherals.settle(&mut context);
        self.update_interrupt_lines();
    }

    /// The interrupt lines held high, one bit per IRQ number, if any changed since the last call.
    pub fn take_interrupt_lines(&mut self) -> Option<u64> {
        self.interrupts.take_if_changed()
    }

    /// The interrupt lines held high now.
    pub fn interrupt_lines(&self) -> u64 {
        self.interrupts.high()
    }

    /// A reset the chip asked for: a watchdog timeout or a wake from System OFF.
    pub fn take_reset_request(&mut self) -> Option<ResetKind> {
        self.signals.reset_request.take()
    }

    /// Whether the CPU should hand back control: a reset is wanted or a watchpoint was hit.
    pub fn stop_requested(&self) -> bool {
        self.signals.reset_request.is_some() || self.watchpoint_hit.is_some()
    }

    /// Resets the chip the way `kind` does: RAM, flash, UICR, GPREGRET and RESETREAS survive all but
    /// a power-on, which fills RAM with noise; the watchdog survives a soft reset and a lockup.
    pub fn reset(&mut self, kind: ResetKind, ports: &mut ChipPorts) {
        self.signals.raised_events.clear();
        self.signals.pin_changes.clear();
        self.signals.reset_request = None;
        let mut context = context!(self, ports);
        self.peripherals.reset(kind, &mut context);
        let reason = match kind {
            ResetKind::PowerOn => 0,
            ResetKind::Pin => RESET_REASON_PIN,
            ResetKind::Watchdog => RESET_REASON_WATCHDOG,
            ResetKind::SoftReset => RESET_REASON_SOFT,
            ResetKind::Lockup => RESET_REASON_LOCKUP,
            ResetKind::WakeFromSystemOff => RESET_REASON_SYSTEM_OFF_GPIO,
        };
        if kind == ResetKind::PowerOn {
            self.peripherals.power_clock.power_cycle();
        } else {
            self.peripherals.power_clock.record_reset_reason(reason);
        }
        self.peripherals.settle(&mut context);
        if kind == ResetKind::PowerOn {
            self.memory.fill_ram_with_noise();
        }
        self.signals.touched = 0;
        self.interrupts.clear();
        self.watchpoint_hit = None;
    }

    pub fn system_off(&self) -> bool {
        self.peripherals.power_clock.system_off()
    }

    // ========================================================================
    // Pins
    // ========================================================================

    /// A level driven from outside the chip, or `None` to let the pin go: input levels, SENSE, LATCH and GPIOTE IN events follow.
    pub fn drive_pin(&mut self, change: PinChange, ports: &mut ChipPorts) {
        self.peripherals.gpio.drive_externally(change.pin, change.level);
        self.settle(ports);
    }

    /// Fits a board resistor to VCC (`Some(true)`) or ground (`Some(false)`), or removes it.
    pub fn set_board_pull(&mut self, pin: u8, pull: Option<bool>, ports: &mut ChipPorts) {
        self.peripherals.gpio.set_board_pull(pin, pull);
        self.settle(ports);
    }

    /// The electrical level of every pin, one bit each; a floating pin reads low.
    pub fn pin_input_levels(&self) -> u32 {
        self.peripherals.gpio.levels()
    }

    /// The levels of the pins the chip itself drives; pins it does not drive read zero.
    pub fn pin_output_levels(&self) -> u32 {
        self.peripherals.gpio.levels() & self.peripherals.gpio.driven_pins()
    }

    /// The pins the chip's own drivers set, from DIR, GPIOTE tasks and the drive mode.
    pub fn pin_outputs(&self) -> u32 {
        self.peripherals.gpio.driven_pins()
    }

    /// How many times the port's DETECT signal has risen since the chip was made.
    pub fn detect_rise_count(&self) -> u64 {
        self.peripherals.gpio.detect_rise_count()
    }

    fn settle(&mut self, ports: &mut ChipPorts) {
        let mut context = context!(self, ports);
        self.peripherals.settle(&mut context);
        self.update_interrupt_lines();
    }

    // ========================================================================
    // Analog and other outside inputs
    // ========================================================================

    /// Sets VDD, which the SAADC reads as an input and as the VDD/4 reference; 3.3 V until set.
    pub fn set_supply_volts(&mut self, volts: f64) {
        self.peripherals.saadc.set_supply_volts(volts);
    }

    /// Sets the die temperature the next TEMP measurement reports; 25 °C until set.
    pub fn set_die_temperature(&mut self, celsius: f64) {
        self.peripherals.temp.set_celsius(celsius);
    }

    /// Sets whether the comparator input is above its reference.
    pub fn set_comparator_input_above(&mut self, above: bool, ports: &mut ChipPorts) {
        let mut context = context!(self, ports);
        self.peripherals.comp_lpcomp.set_input_above(above, &mut context);
        self.peripherals.settle(&mut context);
        self.update_interrupt_lines();
    }

    /// Raises an MWU watch event by hand, by its event bit.
    pub fn trigger_mwu_event(&mut self, bit: u32, ports: &mut ChipPorts) {
        let mut context = context!(self, ports);
        self.peripherals.mwu.trigger_event(bit, &mut context);
        self.peripherals.settle(&mut context);
        self.update_interrupt_lines();
    }

    /// The duty cycle, 0 to 1, of channel `channel` of PWM `instance`; 0 while stopped.
    pub fn pwm_duty_cycle(&self, instance: usize, channel: usize) -> f64 {
        self.peripherals
            .pwms
            .get(instance)
            .map_or(0.0, |pwm| pwm.duty_cycle(channel))
    }

    pub fn pwm_running(&self, instance: usize) -> bool {
        self.peripherals
            .pwms
            .get(instance)
            .is_some_and(|pwm| pwm.running())
    }

    /// The pin channel `channel` of PWM `instance` drives, or `None` when disconnected.
    pub fn pwm_output_pin(&self, instance: usize, channel: usize) -> Option<u8> {
        self.peripherals
            .pwms
            .get(instance)
            .and_then(|pwm| pwm.output_pin(channel))
    }

    /// Bytes arriving on the UART's RX line.
    pub fn uart_receive(&mut self, bytes: &[u8], ports: &mut ChipPorts) {
        self.serial_port_receive(SerialPort::Uart0, bytes, ports);
    }

    /// The bytes the firmware sent on the UART since the last call.
    pub fn take_uart_output(&mut self) -> Vec<u8> {
        self.take_serial_port_output(SerialPort::Uart0)
    }

    /// Bytes arriving on the RX line of `port`; they wait until the firmware arms a receiver.
    pub fn serial_port_receive(&mut self, port: SerialPort, bytes: &[u8], ports: &mut ChipPorts) {
        let mut context = context!(self, ports);
        if let Some(uart) = self.peripherals.serial_port_mut(port) {
            uart.receive(bytes, &mut context);
        }
        self.peripherals.settle(&mut context);
        self.update_interrupt_lines();
    }

    /// How many received bytes `port` holds that the firmware has not taken yet.
    pub fn serial_port_waiting_bytes(&mut self, port: SerialPort) -> usize {
        self.peripherals
            .serial_port_mut(port)
            .map_or(0, |uart| uart.waiting_bytes())
    }

    /// The bytes the firmware sent on `port` since the last call.
    pub fn take_serial_port_output(&mut self, port: SerialPort) -> Vec<u8> {
        self.peripherals
            .serial_port_mut(port)
            .map(|uart| uart.take_output())
            .unwrap_or_default()
    }

    // ========================================================================
    // Radio
    // ========================================================================

    /// Attaches where transmitted packets go, or detaches it with `None`.
    pub fn set_radio_medium(&mut self, medium: Option<Box<dyn RadioMedium>>) {
        self.peripherals.radio.set_medium(medium);
    }

    /// A packet another radio started sending now.
    pub fn receive_radio_frame(&mut self, frame: RadioFrame, ports: &mut ChipPorts) {
        self.peripherals.radio.frame_arrives(frame, ports.now);
        self.settle(ports);
    }

    pub fn radio_transmitted_frames(&self) -> u64 {
        self.peripherals.radio.transmitted_frames()
    }

    /// Whether the radio is disabled with nothing queued, so it cannot send or hear a packet
    /// until the firmware or the PPI starts it again.
    pub fn radio_idle(&self) -> bool {
        self.peripherals.radio.idle()
    }

    // ========================================================================
    // Memory for loaders and the monitor
    // ========================================================================

    /// Flash as the loader sees it: writes here bypass the NVMC.
    pub fn flash(&self) -> &[u8] {
        &self.memory.flash
    }

    pub fn flash_mut(&mut self) -> &mut [u8] {
        &mut self.memory.flash
    }

    pub fn ram(&self) -> &[u8] {
        &self.memory.ram
    }

    pub fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.memory.ram
    }

    pub fn uicr_mut(&mut self) -> &mut [u8] {
        &mut self.memory.uicr
    }

    pub fn ficr_mut(&mut self) -> &mut [u8] {
        &mut self.memory.ficr
    }

    /// A debugger read: memory directly and without watchpoints, peripherals as normal reads.
    pub fn read_for_debugger(
        &mut self,
        address: u32,
        size: AccessSize,
        ports: &mut ChipPorts,
    ) -> Result<u32, BusError> {
        match self.memory.locate(address, size.bytes()) {
            Some((region, offset)) => Ok(Memory::read_value(self.memory.bytes(region), offset, size)),
            None => self.read_peripheral(address, size, ports),
        }
    }

    /// A debugger write: any memory directly, flash and FICR included, and peripherals as normal writes.
    pub fn write_for_debugger(
        &mut self,
        address: u32,
        value: u32,
        size: AccessSize,
        ports: &mut ChipPorts,
    ) -> Result<(), BusError> {
        match self.memory.locate(address, size.bytes()) {
            Some((region, offset)) => {
                Memory::write_value(self.memory.bytes_mut(region), offset, value, size);
                Ok(())
            }
            None => self.write_peripheral(address, value, size, ports),
        }
    }

    // ========================================================================
    // Watchpoints
    // ========================================================================

    /// Watches reads and writes of `size` bytes at `address`.
    pub fn add_watchpoint(&mut self, address: u32, size: u32) {
        self.add_watchpoint_with_access(address, size, WatchAccess::ReadWrite);
    }

    pub fn add_watchpoint_with_access(&mut self, address: u32, size: u32, access: WatchAccess) {
        self.watchpoints.push(Watchpoint {
            address,
            size: size.max(1),
            access,
        });
    }

    pub fn remove_watchpoint(&mut self, address: u32) {
        self.watchpoints
            .retain(|watchpoint| watchpoint.address != address);
    }

    /// The first watched access since the last call: its address, the value read or written, and whether it was a write.
    pub fn take_watchpoint_hit(&mut self) -> Option<(u32, u32, bool)> {
        self.watchpoint_hit.take()
    }

    fn check_watchpoints(&mut self, address: u32, size: AccessSize, value: u32, write: bool) {
        if self.watchpoint_hit.is_some() {
            return;
        }
        let end = address as u64 + size.bytes() as u64;
        let hit = self.watchpoints.iter().any(|watchpoint| {
            let wanted = match watchpoint.access {
                WatchAccess::Read => !write,
                WatchAccess::Write => write,
                WatchAccess::ReadWrite => true,
            };
            wanted
                && (address as u64) < watchpoint.address as u64 + watchpoint.size as u64
                && end > watchpoint.address as u64
        });
        if hit {
            self.watchpoint_hit = Some((address, value & size.mask(), write));
        }
    }

    // ========================================================================
    // Debugging aids
    // ========================================================================

    /// Records every event a peripheral raises, with its cycle, until turned off.
    pub fn set_event_trace(&mut self, enabled: bool) {
        self.signals.event_trace = enabled.then(Vec::new);
    }

    /// The traced events since the last call: each one's cycle and event register address.
    pub fn take_event_trace(&mut self) -> Vec<(Cycle, u32)> {
        self.signals
            .event_trace
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Accesses to registers no model describes, which keep what is written.
    pub fn unhandled_access_count(&self) -> u64 {
        self.peripherals.unhandled_accesses
    }

    /// CPU writes to flash or UICR the NVMC refused.
    pub fn ignored_flash_write_count(&self) -> u64 {
        self.peripherals.nvmc.ignored_writes()
    }

    /// Chains of PPI tasks cut off because they never ended.
    pub fn cut_off_chain_count(&self) -> u64 {
        self.peripherals.cut_off_chains
    }
}
