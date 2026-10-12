// SPDX-License-Identifier: GPL-3.0-only
//! MRS, MSR, CPS and the hints, including the WFI, WFE and SEV sleep instructions.
//! Ported from InfiniEmu's cpu.c.

use crate::cpu::{CortexM4, Sleep};
use crate::decode::HintKind;

// ============================================================================
// Special register numbers (SYSm)
// ============================================================================

const APSR_LAST: u8 = 7;
const MAIN_STACK_POINTER: u8 = 8;
const PROCESS_STACK_POINTER: u8 = 9;
const PRIMASK: u8 = 16;
const BASEPRI: u8 = 17;
const BASEPRI_MAX: u8 = 18;
const FAULTMASK: u8 = 19;
const CONTROL: u8 = 20;

impl CortexM4 {
    pub(crate) fn execute_move_from_special(&mut self, destination: u8, special: u8) -> u32 {
        let privileged = self.is_privileged();
        let value = match special {
            0..=APSR_LAST => {
                let mut value = 0;
                if special & 1 != 0 {
                    value |= self.exception_number as u32;
                }
                if special & 4 == 0 {
                    value |= self.flags.to_bits();
                }
                value
            }
            MAIN_STACK_POINTER if privileged => self.main_stack_pointer(),
            PROCESS_STACK_POINTER if privileged => self.process_stack_pointer(),
            PRIMASK => self.primask as u32,
            BASEPRI | BASEPRI_MAX => self.basepri as u32,
            FAULTMASK => self.faultmask as u32,
            CONTROL => self.control.to_bits(),
            _ => 0,
        };
        self.write_register(destination, value);
        1
    }

    pub(crate) fn execute_move_to_special(&mut self, source: u8, special: u8, mask: u8) -> u32 {
        let value = self.read_register(source);
        if special <= APSR_LAST {
            if special & 4 == 0 {
                self.flags.set_from_bits(value, mask & 2 != 0, mask & 1 != 0);
            }
            return 1;
        }
        if !self.is_privileged() {
            return 1;
        }
        let priority_mask = self.exceptions.priority_mask;
        match special {
            MAIN_STACK_POINTER => self.set_main_stack_pointer(value),
            PROCESS_STACK_POINTER => self.set_process_stack_pointer(value),
            PRIMASK => {
                self.primask = value & 1 != 0;
                self.refresh_execution_priority();
            }
            BASEPRI => {
                self.basepri = value as u8 & priority_mask;
                self.refresh_execution_priority();
            }
            BASEPRI_MAX => {
                let new = value as u8 & priority_mask;
                if new != 0 && (new < self.basepri || self.basepri == 0) {
                    self.basepri = new;
                    self.refresh_execution_priority();
                }
            }
            FAULTMASK => {
                if self.exceptions.execution_priority > -1 || value & 1 == 0 {
                    self.faultmask = value & 1 != 0;
                    self.refresh_execution_priority();
                }
            }
            CONTROL => {
                self.control.unprivileged = value & 1 != 0;
                if !self.in_handler_mode() {
                    self.control.process_stack = value & 2 != 0;
                    self.select_stack_pointer();
                }
                self.control.floating_point_active = value & 4 != 0;
            }
            _ => {}
        }
        1
    }

    pub(crate) fn execute_change_processor_state(
        &mut self,
        disable: bool,
        primask: bool,
        faultmask: bool,
    ) -> u32 {
        if !self.is_privileged() {
            return 1;
        }
        if primask {
            self.primask = disable;
        }
        if faultmask && (!disable || self.exceptions.execution_priority > -1) {
            self.faultmask = disable;
        }
        self.refresh_execution_priority();
        1
    }

    pub(crate) fn execute_hint(&mut self, kind: HintKind) -> u32 {
        match kind {
            HintKind::WaitForInterrupt => self.sleep = Sleep::WaitForInterrupt,
            HintKind::WaitForEvent => {
                if self.event_register {
                    self.event_register = false;
                } else {
                    self.sleep = Sleep::WaitForEvent;
                }
            }
            HintKind::SendEvent => self.event_register = true,
            HintKind::Nop | HintKind::Yield | HintKind::Barrier | HintKind::Preload => {}
        }
        1
    }
}
