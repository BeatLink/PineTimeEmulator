// SPDX-License-Identifier: GPL-3.0-only
//! The programmable peripheral interconnect: 20 channels the firmware wires, 12 fixed ones, forks
//! and channel groups. Each event's tasks come back to the chip, which runs them at once.

// ============================================================================
// Registers
// ============================================================================

const TASKS_CHG_EN: u32 = 0x000;
const TASKS_CHG_END: u32 = 0x030;
const CHEN: u32 = 0x500;
const CHENSET: u32 = 0x504;
const CHENCLR: u32 = 0x508;
const CH_EEP: u32 = 0x510;
const CH_END: u32 = 0x5B0;
const CHG: u32 = 0x800;
const CHG_END: u32 = 0x818;
const FORK_TEP: u32 = 0x910;
const FORK_END: u32 = 0x990;

const CHANNEL_COUNT: usize = 32;
const PROGRAMMABLE_CHANNEL_COUNT: usize = 20;
const GROUP_COUNT: usize = 6;

const RADIO: u32 = 0x4000_1000;
const TIMER0: u32 = 0x4000_8000;
const RTC0: u32 = 0x4000_B000;
const CCM_AAR: u32 = 0x4000_F000;

/// The pre-programmed channels 20..31: event and task addresses from the product specification.
const FIXED_CHANNELS: [(u32, u32); 12] = [
    (TIMER0 + 0x140, RADIO),
    (TIMER0 + 0x140, RADIO + 0x004),
    (TIMER0 + 0x144, RADIO + 0x010),
    (RADIO + 0x128, CCM_AAR),
    (RADIO + 0x100, CCM_AAR),
    (RADIO + 0x104, CCM_AAR + 0x004),
    (RADIO + 0x104, TIMER0 + 0x044),
    (RADIO + 0x10C, TIMER0 + 0x048),
    (RTC0 + 0x140, RADIO),
    (RTC0 + 0x140, RADIO + 0x004),
    (RTC0 + 0x140, TIMER0 + 0x00C),
    (RTC0 + 0x140, TIMER0),
];

// ============================================================================
// PPI
// ============================================================================

pub(crate) struct Ppi {
    enabled: u32,
    event_endpoints: [u32; CHANNEL_COUNT],
    task_endpoints: [u32; CHANNEL_COUNT],
    fork_endpoints: [u32; CHANNEL_COUNT],
    groups: [u32; GROUP_COUNT],
}

impl Ppi {
    pub fn new() -> Ppi {
        let mut ppi = Ppi {
            enabled: 0,
            event_endpoints: [0; CHANNEL_COUNT],
            task_endpoints: [0; CHANNEL_COUNT],
            fork_endpoints: [0; CHANNEL_COUNT],
            groups: [0; GROUP_COUNT],
        };
        ppi.reset();
        ppi
    }

    pub fn reset(&mut self) {
        self.enabled = 0;
        self.event_endpoints = [0; CHANNEL_COUNT];
        self.task_endpoints = [0; CHANNEL_COUNT];
        self.fork_endpoints = [0; CHANNEL_COUNT];
        self.groups = [0; GROUP_COUNT];
        for (index, (event, task)) in FIXED_CHANNELS.iter().enumerate() {
            self.event_endpoints[PROGRAMMABLE_CHANNEL_COUNT + index] = *event;
            self.task_endpoints[PROGRAMMABLE_CHANNEL_COUNT + index] = *task;
        }
    }

    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            CHEN | CHENSET | CHENCLR => self.enabled,
            CH_EEP..CH_END if offset.is_multiple_of(4) => {
                let channel = ((offset - CH_EEP) / 8) as usize;
                if (offset - CH_EEP).is_multiple_of(8) {
                    self.event_endpoints[channel]
                } else {
                    self.task_endpoints[channel]
                }
            }
            CHG..CHG_END if offset.is_multiple_of(4) => self.groups[((offset - CHG) / 4) as usize],
            FORK_TEP..FORK_END if offset.is_multiple_of(4) => {
                self.fork_endpoints[((offset - FORK_TEP) / 4) as usize]
            }
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        match offset {
            TASKS_CHG_EN..TASKS_CHG_END if offset.is_multiple_of(4) && value & 1 != 0 => {
                let group = self.groups[((offset - TASKS_CHG_EN) / 8) as usize];
                if (offset - TASKS_CHG_EN).is_multiple_of(8) {
                    self.enabled |= group;
                } else {
                    self.enabled &= !group;
                }
            }
            CHEN => self.enabled = value,
            CHENSET => self.enabled |= value,
            CHENCLR => self.enabled &= !value,
            CH_EEP..CH_END if offset.is_multiple_of(4) => {
                let channel = ((offset - CH_EEP) / 8) as usize;
                if (offset - CH_EEP).is_multiple_of(8) {
                    self.event_endpoints[channel] = value;
                } else {
                    self.task_endpoints[channel] = value;
                }
            }
            CHG..CHG_END if offset.is_multiple_of(4) => self.groups[((offset - CHG) / 4) as usize] = value,
            FORK_TEP..FORK_END if offset.is_multiple_of(4) => {
                self.fork_endpoints[((offset - FORK_TEP) / 4) as usize] = value;
            }
            _ => {}
        }
    }

    /// Adds the task addresses wired to the event at `event_address` to `tasks`, in channel order.
    pub fn tasks_for_event(&self, event_address: u32, tasks: &mut Vec<u32>) {
        let mut channels = self.enabled;
        while channels != 0 {
            let channel = channels.trailing_zeros() as usize;
            channels &= channels - 1;
            if self.event_endpoints[channel] != event_address {
                continue;
            }
            if self.task_endpoints[channel] != 0 {
                tasks.push(self.task_endpoints[channel]);
            }
            if self.fork_endpoints[channel] != 0 {
                tasks.push(self.fork_endpoints[channel]);
            }
        }
    }

    pub fn has_enabled_channels(&self) -> bool {
        self.enabled != 0
    }
}
