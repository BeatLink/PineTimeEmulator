// SPDX-License-Identifier: GPL-3.0-only
//! The core: its state, reset, and the loop that runs instructions and takes exceptions.
//! Ported from InfiniEmu's cpu.c.

use pinetime_core::{AccessSize, Bus};

use crate::debug::Debug;
use crate::decode::cache::DecodeCache;
use crate::exceptions::ENTRY_CYCLES;
use crate::execute::{Fault, Trap};
use crate::fpu::FloatingPoint;
use crate::nvic::{ExceptionState, FIRST_EXTERNAL};
use crate::registers::{Control, Flags, LINK_REGISTER, PROGRAM_COUNTER, STACK_POINTER};
use crate::scb::SystemControl;
use crate::systick::SysTick;

// ============================================================================
// Public types
// ============================================================================

/// Why `run` or `step` handed control back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The cycle budget ran out (for `step`: the one instruction or exception entry finished).
    Deadline,
    /// The core sleeps in WFI or WFE with nothing to wake it.
    Sleeping,
    /// A fault happened where not even HardFault could be taken.
    Lockup,
    /// `Bus::stop_requested` asked for control back.
    StopRequested,
    /// The program wrote AIRCR.SYSRESETREQ; `take_system_reset_request` collects it.
    ResetRequested,
    /// The next instruction sits on a breakpoint, or is a BKPT instruction.
    Breakpoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunOutcome {
    pub cycles: u64,
    pub reason: StopReason,
}

/// The fault registers, for crash reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FaultStatus {
    pub configurable_fault_status: u32,
    pub hard_fault_status: u32,
    pub bus_fault_address: u32,
    pub memory_manage_address: u32,
    pub locked_up: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sleep {
    Awake,
    WaitForInterrupt,
    WaitForEvent,
}

// ============================================================================
// State
// ============================================================================

pub struct CortexM4 {
    /// r0..r15; r13 is whichever stack pointer is in use, r15 reads as the instruction plus 4.
    pub(crate) registers: [u32; 16],
    /// The stack pointer not in use.
    pub(crate) other_stack_pointer: u32,
    pub(crate) on_process_stack: bool,
    /// The address of the next instruction.
    pub(crate) pc: u32,
    pub(crate) flags: Flags,
    pub(crate) it_state: u8,
    pub(crate) thumb: bool,
    /// IPSR: the exception being handled, 0 in thread mode.
    pub(crate) exception_number: u16,
    pub(crate) primask: bool,
    pub(crate) faultmask: bool,
    pub(crate) basepri: u8,
    pub(crate) control: Control,
    pub(crate) fpu: FloatingPoint,
    pub(crate) exceptions: ExceptionState,
    pub(crate) scb: SystemControl,
    pub(crate) systick: SysTick,
    pub(crate) debug: Debug,
    decode_cache: DecodeCache,
    pub(crate) exclusive_monitor_open: bool,
    pub(crate) event_register: bool,
    pub(crate) sleep: Sleep,
    pub(crate) locked_up: bool,
    /// Cycles executed since the core was created; SysTick and the DWT count these.
    pub(crate) cycles: u64,
    executed_instructions: u64,
    /// The instruction word fetched last, so straight-line code pays for each word only once.
    last_fetched_word: u32,
    /// Cycles the memory held data accesses for during the instruction being executed.
    pub(crate) data_wait_cycles: u32,
    pub(crate) system_reset_requested: bool,
    breakpoints: Vec<u32>,
    /// The address `run` stopped at for a breakpoint, which it steps past when resumed.
    resume_breakpoint: Option<u32>,
    /// The external interrupt lines as the bus last reported them.
    pub(crate) interrupt_lines: u64,
    reset_vector_table_offset: u32,
}

impl Default for CortexM4 {
    fn default() -> Self {
        CortexM4::new()
    }
}

impl CortexM4 {
    /// A core with 48 external interrupts, 3 priority bits and the FPU; call `reset` before use.
    pub fn new() -> CortexM4 {
        CortexM4 {
            registers: [0; 16],
            other_stack_pointer: 0,
            on_process_stack: false,
            pc: 0,
            flags: Flags::default(),
            it_state: 0,
            thumb: true,
            exception_number: 0,
            primask: false,
            faultmask: false,
            basepri: 0,
            control: Control::default(),
            fpu: FloatingPoint::new(),
            exceptions: ExceptionState::new(),
            scb: SystemControl::new(0),
            systick: SysTick::new(),
            debug: Debug::new(),
            decode_cache: DecodeCache::new(),
            exclusive_monitor_open: false,
            event_register: false,
            sleep: Sleep::Awake,
            locked_up: false,
            cycles: 0,
            executed_instructions: 0,
            last_fetched_word: u32::MAX,
            data_wait_cycles: 0,
            system_reset_requested: false,
            breakpoints: Vec::new(),
            resume_breakpoint: None,
            interrupt_lines: 0,
            reset_vector_table_offset: 0,
        }
    }

    // ========================================================================
    // Reset
    // ========================================================================

    /// Resets the core, NVIC, SCB, SysTick and DWT, then loads SP and PC from the vector table.
    pub fn reset(&mut self, bus: &mut impl Bus) {
        self.registers = [0; 16];
        self.registers[LINK_REGISTER] = 0xFFFF_FFFF;
        self.other_stack_pointer = 0;
        self.on_process_stack = false;
        self.flags = Flags::default();
        self.it_state = 0;
        self.thumb = true;
        self.exception_number = 0;
        self.primask = false;
        self.faultmask = false;
        self.basepri = 0;
        self.control = Control::default();
        self.fpu = FloatingPoint::new();
        self.exceptions = ExceptionState::new();
        self.scb = SystemControl::new(self.reset_vector_table_offset);
        self.systick = SysTick::new();
        self.debug = Debug::new();
        self.exclusive_monitor_open = false;
        self.event_register = false;
        self.sleep = Sleep::Awake;
        self.locked_up = false;
        self.system_reset_requested = false;
        self.resume_breakpoint = None;
        self.refresh_execution_priority();
        self.repend_held_lines();

        let table = self.scb.vector_table_offset;
        let stack = bus.read(table, AccessSize::Word);
        let entry = bus.read(table.wrapping_add(4), AccessSize::Word);
        match (stack, entry) {
            (Ok(stack), Ok(entry)) => {
                self.registers[STACK_POINTER] = stack & !3;
                self.pc = entry & !1;
                self.thumb = entry & 1 != 0;
            }
            _ => self.enter_lockup(),
        }
    }

    // ========================================================================
    // Running
    // ========================================================================

    /// Runs until `until_cycles` cycles have been executed or something stops the core.
    pub fn run(&mut self, bus: &mut impl Bus, until_cycles: u64) -> RunOutcome {
        self.poll_interrupt_lines(bus);
        let mut cycles = 0u64;
        loop {
            if (self.sleep != Sleep::Awake) | self.locked_up {
                if let Some(reason) = self.halted_reason() {
                    return RunOutcome { cycles, reason };
                }
            }
            if cycles >= until_cycles {
                return RunOutcome {
                    cycles,
                    reason: StopReason::Deadline,
                };
            }
            let (cost, stop) = self.step_once(bus);
            cycles += cost as u64;
            if let Some(reason) = self.after_step(bus, cost, stop) {
                return RunOutcome { cycles, reason };
            }
        }
    }

    /// Runs one instruction or one exception entry.
    pub fn step(&mut self, bus: &mut impl Bus) -> RunOutcome {
        self.poll_interrupt_lines(bus);
        if let Some(reason) = self.halted_reason() {
            return RunOutcome { cycles: 0, reason };
        }
        let (cost, stop) = self.step_once(bus);
        let mut reason = self.after_step(bus, cost, stop).unwrap_or(StopReason::Deadline);
        if self.locked_up {
            reason = StopReason::Lockup;
        }
        RunOutcome {
            cycles: cost as u64,
            reason,
        }
    }

    /// Tells the bus the time that passed and picks up interrupt lines and stop requests.
    #[inline]
    fn after_step<B: Bus>(&mut self, bus: &mut B, cost: u32, stop: Option<StopReason>) -> Option<StopReason> {
        if cost != 0 {
            bus.advance(cost);
        }
        self.poll_interrupt_lines(bus);
        if stop.is_some() {
            return stop;
        }
        if self.system_reset_requested {
            return Some(StopReason::ResetRequested);
        }
        if bus.stop_requested() {
            return Some(StopReason::StopRequested);
        }
        None
    }

    #[inline]
    fn poll_interrupt_lines<B: Bus>(&mut self, bus: &mut B) {
        if let Some(lines) = bus.interrupt_lines() {
            self.update_interrupt_lines(lines);
        }
    }

    /// Runs one instruction or takes one exception; returns its cycles and any reason to stop.
    #[inline(always)]
    fn step_once<B: Bus>(&mut self, bus: &mut B) -> (u32, Option<StopReason>) {
        if self.exceptions.check_needed {
            if let Some(result) = self.take_pending_exception(bus) {
                return result;
            }
        }

        let address = self.pc;
        let mut resuming = false;
        if !self.breakpoints.is_empty() || self.resume_breakpoint.is_some() {
            match self.check_breakpoint(address) {
                Ok(resumed) => resuming = resumed,
                Err(stop) => return stop,
            }
        }

        if !self.thumb || address >= crate::private_bus::START {
            return self.fetch_fault(bus, address);
        }
        let (instruction, length, branch_condition) =
            match self.decode_cache.lookup(address, |at| bus.fetch(at)) {
                Some(decoded) => (decoded.instruction, decoded.length, decoded.condition),
                None => return self.fetch_fault(bus, address),
            };

        let in_it_block = self.it_state != 0;
        let condition = if in_it_block {
            self.it_state >> 4
        } else {
            branch_condition
        };
        self.registers[PROGRAM_COUNTER] = address.wrapping_add(4);
        self.pc = address.wrapping_add(length as u32);
        self.executed_instructions += 1;
        let fetch_wait = self.fetch_wait(bus, address, length as u32);

        if condition < 14 && !self.flags.condition_passed(condition) {
            if in_it_block {
                self.advance_it_state();
            }
            return self.account(1 + fetch_wait, None);
        }

        self.data_wait_cycles = 0;
        match self.execute(bus, instruction) {
            Ok(cycles) => {
                if in_it_block {
                    self.advance_it_state();
                }
                self.account(cycles + fetch_wait + self.data_wait_cycles, None)
            }
            Err(trap) => self.handle_trap(bus, trap, address, in_it_block, resuming),
        }
    }

    #[cold]
    #[inline(never)]
    fn take_pending_exception<B: Bus>(&mut self, bus: &mut B) -> Option<(u32, Option<StopReason>)> {
        self.exceptions.check_needed = false;
        let number = self.preempting_exception()?;
        self.enter_exception(bus, number);
        self.exceptions.check_needed = true;
        self.resume_breakpoint = None;
        Some(self.account(ENTRY_CYCLES, None))
    }

    /// Stops at a breakpoint on `address`; otherwise says whether `run` is resuming from one there.
    #[cold]
    #[inline(never)]
    fn check_breakpoint(&mut self, address: u32) -> Result<bool, (u32, Option<StopReason>)> {
        let resuming = self.resume_breakpoint.take() == Some(address);
        if !resuming && self.breakpoints.contains(&address) {
            self.resume_breakpoint = Some(address);
            return Err((0, Some(StopReason::Breakpoint)));
        }
        Ok(resuming)
    }

    /// The fault for an instruction that cannot be fetched or may not run.
    #[cold]
    #[inline(never)]
    fn fetch_fault<B: Bus>(&mut self, bus: &mut B, address: u32) -> (u32, Option<StopReason>) {
        let fault = if !self.thumb {
            Fault::InvalidState
        } else if address >= crate::private_bus::START {
            Fault::InstructionAccess
        } else {
            Fault::InstructionBus
        };
        self.raise_fault(bus, fault);
        self.account(ENTRY_CYCLES, None)
    }

    #[cold]
    #[inline(never)]
    fn handle_trap<B: Bus>(
        &mut self,
        bus: &mut B,
        trap: Trap,
        address: u32,
        in_it_block: bool,
        resuming: bool,
    ) -> (u32, Option<StopReason>) {
        match trap {
            Trap::Fault(fault) => {
                self.pc = address;
                self.raise_fault(bus, fault);
                self.account(ENTRY_CYCLES, None)
            }
            Trap::ExceptionReturn(value) => match self.return_from_exception(bus, value) {
                Ok(cycles) => self.account(cycles, None),
                Err(Trap::Fault(fault)) => {
                    self.pc = address;
                    self.raise_fault(bus, fault);
                    self.account(ENTRY_CYCLES, None)
                }
                Err(_) => self.account(1, None),
            },
            Trap::SupervisorCall => {
                if in_it_block {
                    self.advance_it_state();
                }
                self.supervisor_call(bus);
                self.account(1 + ENTRY_CYCLES, None)
            }
            Trap::Breakpoint => {
                if resuming {
                    if in_it_block {
                        self.advance_it_state();
                    }
                    return self.account(1, None);
                }
                self.pc = address;
                self.executed_instructions -= 1;
                self.resume_breakpoint = Some(address);
                (0, Some(StopReason::Breakpoint))
            }
        }
    }

    /// The cycles the memory holds the CPU for to fetch an instruction, counting each word once
    /// while code runs straight on, as the core's prefetch reads whole words.
    #[inline(always)]
    fn fetch_wait<B: Bus>(&mut self, bus: &mut B, address: u32, length: u32) -> u32 {
        let first = address & !3;
        let last = address.wrapping_add(length - 1) & !3;
        let mut wait = 0;
        if first != self.last_fetched_word {
            wait += bus.fetch_wait_cycles(first);
        }
        if last != first {
            wait += bus.fetch_wait_cycles(last);
        }
        self.last_fetched_word = last;
        wait
    }

    /// Counts `cycles` as executed and fires SysTick if it reached zero.
    #[inline]
    fn account(&mut self, cycles: u32, stop: Option<StopReason>) -> (u32, Option<StopReason>) {
        self.cycles += cycles as u64;
        if self.cycles >= self.systick.zero_at {
            self.system_tick_reached_zero();
        }
        (cycles, stop)
    }

    // ========================================================================
    // Sleep
    // ========================================================================

    /// Lockup, or sleep with nothing to wake the core.
    #[cold]
    #[inline(never)]
    fn halted_reason(&mut self) -> Option<StopReason> {
        if self.locked_up {
            return Some(StopReason::Lockup);
        }
        if self.sleep != Sleep::Awake && !self.try_wake() {
            return Some(StopReason::Sleeping);
        }
        None
    }

    /// An exception would preempt if PRIMASK were clear, which is what wakes WFI.
    fn exception_would_wake(&self) -> bool {
        match self.highest_pending_exception() {
            Some((_, group_level)) => group_level < self.compute_execution_priority(false),
            None => false,
        }
    }

    /// Leaves sleep if something wakes the core now.
    fn try_wake(&mut self) -> bool {
        if self.sleep == Sleep::WaitForEvent && self.event_register {
            self.event_register = false;
            self.sleep = Sleep::Awake;
            return true;
        }
        if self.exception_would_wake() {
            self.sleep = Sleep::Awake;
            return true;
        }
        false
    }

    // ========================================================================
    // Public accessors
    // ========================================================================

    pub fn is_sleeping(&self) -> bool {
        self.sleep != Sleep::Awake
    }

    /// Whether an enabled interrupt or (for WFE) an event would wake the core now.
    pub fn wake_reason_pending(&self) -> bool {
        let event = self.sleep != Sleep::WaitForInterrupt && self.event_register;
        event || self.exception_would_wake()
    }

    /// Whether AIRCR.SYSRESETREQ was written since the last call.
    pub fn take_system_reset_request(&mut self) -> bool {
        std::mem::take(&mut self.system_reset_requested)
    }

    /// r0..r15; r15 is the address of the next instruction.
    pub fn register(&self, number: usize) -> u32 {
        match number {
            PROGRAM_COUNTER => self.pc,
            _ => self.registers[number],
        }
    }

    pub fn set_register(&mut self, number: usize, value: u32) {
        match number {
            PROGRAM_COUNTER => self.set_pc(value),
            STACK_POINTER => self.set_sp(value),
            _ => self.registers[number] = value,
        }
    }

    pub fn pc(&self) -> u32 {
        self.pc
    }

    pub fn set_pc(&mut self, value: u32) {
        self.pc = value & !1;
        self.thumb = true;
        self.it_state = 0;
    }

    pub fn sp(&self) -> u32 {
        self.registers[STACK_POINTER]
    }

    pub fn set_sp(&mut self, value: u32) {
        self.registers[STACK_POINTER] = value & !3;
    }

    pub fn vector_table_offset(&self) -> u32 {
        self.scb.vector_table_offset
    }

    /// Sets VTOR now and the value `reset` gives it from now on.
    pub fn set_vector_table_offset(&mut self, value: u32) {
        self.scb.vector_table_offset = value & 0xFFFF_FF80;
        self.reset_vector_table_offset = self.scb.vector_table_offset;
    }

    pub fn xpsr(&self) -> u32 {
        self.program_status()
    }

    pub fn executed_instructions(&self) -> u64 {
        self.executed_instructions
    }

    /// The cycles executed since the core was created.
    pub fn cycles(&self) -> u64 {
        self.cycles
    }

    pub fn set_breakpoint(&mut self, address: u32) {
        let address = address & !1;
        if !self.breakpoints.contains(&address) {
            self.breakpoints.push(address);
        }
    }

    pub fn clear_breakpoint(&mut self, address: u32) {
        let address = address & !1;
        self.breakpoints.retain(|&existing| existing != address);
    }

    pub fn fault_status(&self) -> FaultStatus {
        FaultStatus {
            configurable_fault_status: self.scb.configurable_fault_status,
            hard_fault_status: self.scb.hard_fault_status,
            bus_fault_address: self.scb.bus_fault_address,
            memory_manage_address: self.scb.memory_manage_address,
            locked_up: self.locked_up,
        }
    }

    /// Marks external interrupt `number` pending, as a write to NVIC_ISPR would.
    pub fn set_interrupt_pending(&mut self, number: usize) {
        if number < crate::nvic::EXTERNAL_INTERRUPT_COUNT {
            self.pend_external_interrupts(1 << number);
        }
    }

    /// Takes the level of every interrupt line at once, as `Bus::interrupt_lines` reports them.
    pub fn set_interrupt_lines(&mut self, lines: u64) {
        self.update_interrupt_lines(lines);
    }

    /// An event from outside the core: sets the event register, which wakes WFE.
    pub fn send_event(&mut self) {
        self.event_register = true;
    }

    /// Whether external interrupt `number` is pending.
    pub fn is_interrupt_pending(&self, number: usize) -> bool {
        self.exceptions.pending & (1u64 << (FIRST_EXTERNAL + number)) != 0
    }

    /// The single-precision register s`number` as raw bits.
    pub fn float_register(&self, number: usize) -> u32 {
        self.fpu.registers[number]
    }

    pub fn set_float_register(&mut self, number: usize, value: u32) {
        self.fpu.registers[number] = value;
    }
}
