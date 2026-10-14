// SPDX-License-Identifier: GPL-3.0-only
//! A whole device: the CPU, the chip and its board, run against one emulated clock. The watch is
//! a `Machine` with the PineTime's board; the Bluetooth bridge uses the same machine with its own.

use pinetime_board::{AccelerometerModel, PineTimeBoard};
use pinetime_core::{
    AccessSize, BoardPorts, Bus, BusError, Cycle, DeviceContext, EventKey, Owner, PinChange, QueuedContext,
    Scheduler,
};
use pinetime_cortex_m4::{CortexM4, StopReason};
use pinetime_nrf52832::{ChipPorts, Nrf52832, RadioFrame, ResetKind, SerialPort};

// ============================================================================
// Boards
// ============================================================================

/// The parts around the chip, which a power-on puts in their idle state.
pub trait Board: BoardPorts {
    fn power_on(&mut self, context: &mut dyn DeviceContext);
}

impl Board for PineTimeBoard {
    fn power_on(&mut self, context: &mut dyn DeviceContext) {
        PineTimeBoard::power_on(self, context);
    }
}

// ============================================================================
// Machine
// ============================================================================

pub struct Machine<B: Board = PineTimeBoard> {
    pub cpu: CortexM4,
    pub chip: Nrf52832,
    pub board: B,
    pub scheduler: Scheduler,
    now: Cycle,
    locked_up: bool,
    /// The CPU takes its stack and reset address from the vector table when it first runs, so
    /// images loaded after power-on are the ones it starts.
    vector_table_unread: bool,
    pending_watchpoint_hits: Vec<WatchpointHit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchpointHit {
    pub address: u32,
    pub value: u32,
    pub write: bool,
    pub program_counter: u32,
}

impl Machine {
    /// The PineTime: an nRF52832 on the watch's board, powered on at time zero.
    pub fn new() -> Machine {
        Machine::with_parts(Nrf52832::new(), PineTimeBoard::new(AccelerometerModel::Bma425), 0)
    }
}

impl<B: Board> Machine<B> {
    /// A machine of `chip` on `board`, powered on with its clock at `start`.
    pub fn with_parts(chip: Nrf52832, board: B, start: Cycle) -> Machine<B> {
        let mut machine = Machine {
            cpu: CortexM4::new(),
            chip,
            board,
            scheduler: Scheduler::new(),
            now: start,
            locked_up: false,
            vector_table_unread: false,
            pending_watchpoint_hits: Vec::new(),
        };
        machine.power_on();
        machine
    }

    pub fn now(&self) -> Cycle {
        self.now
    }

    pub fn locked_up(&self) -> bool {
        self.locked_up
    }

    // ------------------------------------------------------------------------
    // Resets
    // ------------------------------------------------------------------------

    /// Starts the watch as if its battery had just been connected: the board parts too.
    pub fn power_on(&mut self) {
        self.scheduler.clear();
        let mut context = QueuedContext::new(self.now, &mut self.scheduler);
        self.board.power_on(&mut context);
        let pin_changes = context.pin_changes;
        self.reset_chip(ResetKind::PowerOn);
        self.vector_table_unread = true;
        self.apply_pin_changes(pin_changes);
    }

    /// Keeps the registers a debugger set instead of reading the vector table at the first run.
    pub fn keep_cpu_registers(&mut self) {
        self.vector_table_unread = false;
    }

    /// Resets the chip and the CPU the way `kind` does on the real nRF52832; board parts keep running.
    fn reset_cpu_only(&mut self) {
        let mut bus = MachineBus {
            chip: &mut self.chip,
            board: &mut self.board,
            scheduler: &mut self.scheduler,
            now: self.now,
            deadline: Cycle::MAX,
        };
        self.cpu.reset(&mut bus);
    }

    pub fn reset_chip(&mut self, kind: ResetKind) {
        let mut ports = ChipPorts {
            now: self.now,
            scheduler: &mut self.scheduler,
            board: &mut self.board,
        };
        self.chip.reset(kind, &mut ports);
        self.locked_up = false;
        self.reset_cpu_only();
    }

    // ------------------------------------------------------------------------
    // Running
    // ------------------------------------------------------------------------

    /// Runs the watch until emulated time reaches `target`.
    pub fn run_until(&mut self, target: Cycle) {
        if self.vector_table_unread {
            self.vector_table_unread = false;
            self.reset_cpu_only();
        }
        while self.now < target {
            let deadline = self.scheduler.next_deadline().unwrap_or(Cycle::MAX).min(target);
            if deadline > self.now {
                self.run_cpu_until(deadline);
            }
            self.handle_due_events();
            self.handle_requests();
        }
    }

    fn run_cpu_until(&mut self, deadline: Cycle) {
        let cpu_can_run = !self.locked_up && !self.chip.system_off();
        if !cpu_can_run || (self.cpu.is_sleeping() && !self.cpu.wake_reason_pending()) {
            self.now = deadline;
            return;
        }

        let mut bus = MachineBus {
            chip: &mut self.chip,
            board: &mut self.board,
            scheduler: &mut self.scheduler,
            now: self.now,
            deadline,
        };
        let outcome = self.cpu.run(&mut bus, deadline - self.now);
        let (reached, shortened_deadline) = (bus.now, bus.deadline);
        self.now = reached;

        match outcome.reason {
            StopReason::Lockup => self.locked_up = true,
            StopReason::Sleeping if !self.cpu.wake_reason_pending() => {
                self.now = self.now.max(shortened_deadline);
            }
            _ => {}
        }
    }

    /// Runs one instruction or exception entry, for a debugger stepping through firmware.
    pub fn step(&mut self) -> StopReason {
        if self.vector_table_unread {
            self.vector_table_unread = false;
            self.reset_cpu_only();
        }
        let mut bus = MachineBus {
            chip: &mut self.chip,
            board: &mut self.board,
            scheduler: &mut self.scheduler,
            now: self.now,
            deadline: Cycle::MAX,
        };
        let outcome = self.cpu.step(&mut bus);
        self.now = bus.now;
        if outcome.reason == StopReason::Lockup {
            self.locked_up = true;
        }
        self.handle_due_events();
        self.handle_requests();
        outcome.reason
    }

    fn handle_due_events(&mut self) {
        while let Some((_, key)) = self.scheduler.pop_due(self.now) {
            match key.owner {
                Owner::Peripheral(_) => {
                    let mut ports = ChipPorts {
                        now: self.now,
                        scheduler: &mut self.scheduler,
                        board: &mut self.board,
                    };
                    self.chip.handle_event(key, &mut ports);
                }
                Owner::Device(_) => self.handle_board_event(key),
                Owner::Machine => {}
            }
        }
        self.deliver_interrupts();
    }

    fn handle_board_event(&mut self, key: EventKey) {
        let mut context = QueuedContext::new(self.now, &mut self.scheduler);
        self.board.handle_event(key, &mut context);
        let pin_changes = context.pin_changes;
        self.apply_pin_changes(pin_changes);
    }

    fn handle_requests(&mut self) {
        if let Some(kind) = self.chip.take_reset_request() {
            self.reset_chip(kind);
        }
        if self.cpu.take_system_reset_request() {
            self.reset_chip(ResetKind::SoftReset);
        }
        if self.locked_up {
            self.reset_chip(ResetKind::Lockup);
        }
        if let Some((address, value, write)) = self.chip.take_watchpoint_hit() {
            self.pending_watchpoint_hits.push(WatchpointHit {
                address,
                value,
                write,
                program_counter: self.cpu.pc(),
            });
        }
        self.deliver_interrupts();
    }

    fn deliver_interrupts(&mut self) {
        if let Some(lines) = self.chip.take_interrupt_lines() {
            self.cpu.set_interrupt_lines(lines);
        }
    }

    pub fn take_watchpoint_hits(&mut self) -> Vec<WatchpointHit> {
        std::mem::take(&mut self.pending_watchpoint_hits)
    }

    // ------------------------------------------------------------------------
    // Outside inputs
    // ------------------------------------------------------------------------

    /// Lets a board part change in response to an outside input, then applies the pins it moved.
    pub fn with_board<Result>(
        &mut self,
        change: impl FnOnce(&mut B, &mut QueuedContext) -> Result,
    ) -> Result {
        let mut context = QueuedContext::new(self.now, &mut self.scheduler);
        let result = change(&mut self.board, &mut context);
        let pin_changes = context.pin_changes;
        self.apply_pin_changes(pin_changes);
        result
    }

    /// Hands the chip a radio packet another machine started sending.
    pub fn receive_radio_frame(&mut self, frame: RadioFrame) {
        let mut ports = ChipPorts {
            now: self.now,
            scheduler: &mut self.scheduler,
            board: &mut self.board,
        };
        self.chip.receive_radio_frame(frame, &mut ports);
        self.deliver_interrupts();
    }

    /// Hands bytes from outside to the RX line of one of the chip's UARTs.
    pub fn serial_port_receive(&mut self, port: SerialPort, bytes: &[u8]) {
        let mut ports = ChipPorts {
            now: self.now,
            scheduler: &mut self.scheduler,
            board: &mut self.board,
        };
        self.chip.serial_port_receive(port, bytes, &mut ports);
        self.deliver_interrupts();
    }

    /// The earliest time this machine could act on its own: now while the CPU runs, else the next event.
    pub fn quiet_until(&mut self) -> Cycle {
        let cpu_can_run = !self.locked_up && !self.chip.system_off();
        let cpu_waits = !cpu_can_run || (self.cpu.is_sleeping() && !self.cpu.wake_reason_pending());
        if !cpu_waits {
            return self.now;
        }
        self.scheduler.next_deadline().unwrap_or(Cycle::MAX).max(self.now)
    }

    fn apply_pin_changes(&mut self, pin_changes: Vec<PinChange>) {
        for change in pin_changes {
            let mut ports = ChipPorts {
                now: self.now,
                scheduler: &mut self.scheduler,
                board: &mut self.board,
            };
            self.chip.drive_pin(change, &mut ports);
        }
        self.deliver_interrupts();
    }

    // ------------------------------------------------------------------------
    // Debugger access
    // ------------------------------------------------------------------------

    pub fn read_for_debugger(&mut self, address: u32, size: AccessSize) -> Result<u32, BusError> {
        if CortexM4::is_private_bus_address(address) {
            return Ok(self.cpu.read_private_bus_for_debugger(address, size));
        }
        let mut ports = ChipPorts {
            now: self.now,
            scheduler: &mut self.scheduler,
            board: &mut self.board,
        };
        self.chip.read_for_debugger(address, size, &mut ports)
    }

    pub fn write_for_debugger(&mut self, address: u32, value: u32, size: AccessSize) -> Result<(), BusError> {
        if CortexM4::is_private_bus_address(address) {
            self.cpu.write_private_bus_for_debugger(address, value, size);
            return Ok(());
        }
        let mut ports = ChipPorts {
            now: self.now,
            scheduler: &mut self.scheduler,
            board: &mut self.board,
        };
        let result = self.chip.write_for_debugger(address, value, size, &mut ports);
        self.deliver_interrupts();
        result
    }
}

impl Default for Machine {
    fn default() -> Machine {
        Machine::new()
    }
}

// ============================================================================
// The bus the CPU sees
// ============================================================================

/// Everything but the CPU, lent to the CPU while it runs.
struct MachineBus<'a, B: BoardPorts> {
    chip: &'a mut Nrf52832,
    board: &'a mut B,
    scheduler: &'a mut Scheduler,
    now: Cycle,
    deadline: Cycle,
}

const PERIPHERAL_SPACE_START: u32 = 0x4000_0000;

static TRACING_PERIPHERALS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Turns printing every peripheral access on standard error on or off, for debugging firmware.
pub fn trace_peripherals(on: bool) {
    TRACING_PERIPHERALS.store(on, std::sync::atomic::Ordering::Relaxed);
}

#[inline]
fn tracing_peripherals() -> bool {
    TRACING_PERIPHERALS.load(std::sync::atomic::Ordering::Relaxed)
}

fn trace_access(now: Cycle, kind: &str, address: u32, value: Result<u32, BusError>) {
    if address >= PERIPHERAL_SPACE_START && tracing_peripherals() {
        eprintln!("{now} {kind} {address:#010x} {value:x?}");
    }
}

impl<B: BoardPorts> MachineBus<'_, B> {
    /// A peripheral access may queue an event sooner than the CPU was told to run.
    #[inline]
    fn pull_deadline_in(&mut self, address: u32) {
        if address >= PERIPHERAL_SPACE_START {
            if let Some(next) = self.scheduler.next_deadline() {
                self.deadline = self.deadline.min(next);
            }
        }
    }
}

impl<B: BoardPorts> Bus for MachineBus<'_, B> {
    #[inline]
    fn read(&mut self, address: u32, size: AccessSize) -> Result<u32, BusError> {
        let mut ports = ChipPorts {
            now: self.now,
            scheduler: self.scheduler,
            board: self.board,
        };
        let result = self.chip.read(address, size, &mut ports);
        trace_access(self.now, "read", address, result);
        self.pull_deadline_in(address);
        result
    }

    #[inline]
    fn write(&mut self, address: u32, value: u32, size: AccessSize) -> Result<(), BusError> {
        let mut ports = ChipPorts {
            now: self.now,
            scheduler: self.scheduler,
            board: self.board,
        };
        let result = self.chip.write(address, value, size, &mut ports);
        trace_access(self.now, "write", address, result.map(|_| value));
        self.pull_deadline_in(address);
        result
    }

    #[inline]
    fn fetch(&mut self, address: u32) -> Result<u16, BusError> {
        self.chip.fetch(address)
    }

    #[inline]
    fn fetch_wait_cycles(&mut self, address: u32) -> u32 {
        self.chip.fetch_wait_cycles(address)
    }

    #[inline]
    fn data_wait_cycles(&mut self, address: u32) -> u32 {
        self.chip.data_wait_cycles(address)
    }

    #[inline]
    fn advance(&mut self, cycles: u32) {
        self.now += cycles as Cycle;
    }

    #[inline]
    fn interrupt_lines(&mut self) -> Option<u64> {
        self.chip.take_interrupt_lines()
    }

    #[inline]
    fn stop_requested(&self) -> bool {
        self.now >= self.deadline || self.chip.stop_requested()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinetime_core::time::cycles_from_milliseconds;

    const STACK_TOP: u32 = 0x2001_0000;
    const RESET_HANDLER: u32 = 0x100;
    /// `b .`, a branch to itself.
    const ENDLESS_LOOP: u32 = 0xE7FE;

    fn load_endless_program(machine: &mut Machine) {
        machine
            .write_for_debugger(0x0, STACK_TOP, AccessSize::Word)
            .unwrap();
        machine
            .write_for_debugger(0x4, RESET_HANDLER | 1, AccessSize::Word)
            .unwrap();
        machine
            .write_for_debugger(RESET_HANDLER, ENDLESS_LOOP, AccessSize::Halfword)
            .unwrap();
    }

    #[test]
    fn the_cpu_starts_from_an_image_loaded_after_power_on() {
        let mut machine = Machine::new();
        load_endless_program(&mut machine);

        machine.run_until(cycles_from_milliseconds(1));

        assert_eq!(machine.cpu.pc(), RESET_HANDLER);
        assert_eq!(machine.cpu.sp(), STACK_TOP);
        assert!(!machine.locked_up());
    }

    #[test]
    fn registers_a_debugger_set_are_kept_at_the_first_run() {
        let mut machine = Machine::new();
        load_endless_program(&mut machine);
        machine
            .write_for_debugger(0x200, ENDLESS_LOOP, AccessSize::Halfword)
            .unwrap();
        machine.cpu.set_pc(0x200);
        machine.cpu.set_sp(STACK_TOP - 0x100);
        machine.keep_cpu_registers();

        machine.run_until(cycles_from_milliseconds(1));

        assert_eq!(machine.cpu.pc(), 0x200);
        assert_eq!(machine.cpu.sp(), STACK_TOP - 0x100);
    }

    #[test]
    fn emulated_time_reaches_the_target() {
        let mut machine = Machine::new();
        load_endless_program(&mut machine);

        machine.run_until(cycles_from_milliseconds(5));

        assert_eq!(machine.now(), cycles_from_milliseconds(5));
    }
}
