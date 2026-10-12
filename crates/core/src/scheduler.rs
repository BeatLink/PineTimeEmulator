// SPDX-License-Identifier: GPL-3.0-only
//! A queue of future events, each named by who owns it and a number the owner chooses.
//!
//! Scheduling a key that is already queued moves it, so a peripheral keeps one alarm per key
//! without tracking what it queued before. A sleeping CPU skips straight to the next event.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::time::Cycle;

// ============================================================================
// Event names
// ============================================================================

/// Who handles an event when it comes due.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Owner {
    /// An nRF52832 peripheral, by its instance number (bits 12..17 of its base address).
    Peripheral(u8),
    /// A part on the board, by the number the board gives it.
    Device(u8),
    /// The machine itself, for things like pacing against the wall clock.
    Machine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventKey {
    pub owner: Owner,
    pub token: u32,
}

impl EventKey {
    pub const fn new(owner: Owner, token: u32) -> EventKey {
        EventKey { owner, token }
    }
}

// ============================================================================
// Scheduler
// ============================================================================

/// The queue is compacted once it holds more than this many entries and most of them are stale.
const COMPACT_ABOVE: usize = 64;

#[derive(Default)]
pub struct Scheduler {
    queue: BinaryHeap<Reverse<(Cycle, u64, EventKey)>>,
    live: HashMap<EventKey, u64>,
    next_generation: u64,
}

impl Scheduler {
    pub fn new() -> Scheduler {
        Scheduler::default()
    }

    /// Queues `key` to come due at `cycle`, replacing any earlier time queued for the same key.
    pub fn schedule(&mut self, key: EventKey, cycle: Cycle) {
        let generation = self.next_generation;
        self.next_generation += 1;
        self.live.insert(key, generation);
        self.queue.push(Reverse((cycle, generation, key)));
        if self.queue.len() > COMPACT_ABOVE && self.queue.len() > 2 * self.live.len() {
            self.drop_every_stale_entry();
        }
    }

    /// Rebuilds the queue from the live entries, so moved alarms that lie far ahead do not pile up.
    fn drop_every_stale_entry(&mut self) {
        let live = &self.live;
        self.queue
            .retain(|Reverse((_, generation, key))| live.get(key) == Some(generation));
    }

    /// How many entries the queue holds, stale ones included; for tests.
    pub fn queued_entries(&self) -> usize {
        self.queue.len()
    }

    pub fn cancel(&mut self, key: EventKey) {
        self.live.remove(&key);
    }

    pub fn is_scheduled(&self, key: EventKey) -> bool {
        self.live.contains_key(&key)
    }

    /// The cycle the earliest queued event comes due, if any is queued.
    pub fn next_deadline(&mut self) -> Option<Cycle> {
        self.drop_stale_entries();
        self.queue.peek().map(|Reverse((cycle, _, _))| *cycle)
    }

    /// Takes the earliest event that is due at or before `now`; events due together leave in the
    /// order they were scheduled.
    pub fn pop_due(&mut self, now: Cycle) -> Option<(Cycle, EventKey)> {
        self.drop_stale_entries();
        let Reverse((cycle, _, key)) = *self.queue.peek()?;
        if cycle > now {
            return None;
        }
        self.queue.pop();
        self.live.remove(&key);
        Some((cycle, key))
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.live.clear();
    }

    fn drop_stale_entries(&mut self) {
        while let Some(Reverse((_, generation, key))) = self.queue.peek() {
            if self.live.get(key) == Some(generation) {
                return;
            }
            self.queue.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RTC: EventKey = EventKey::new(Owner::Peripheral(0x11), 0);
    const FLASH: EventKey = EventKey::new(Owner::Device(1), 0);

    #[test]
    fn events_come_due_in_time_order() {
        let mut scheduler = Scheduler::new();
        scheduler.schedule(RTC, 200);
        scheduler.schedule(FLASH, 100);

        assert_eq!(scheduler.next_deadline(), Some(100));
        assert_eq!(scheduler.pop_due(150), Some((100, FLASH)));
        assert_eq!(scheduler.pop_due(150), None);
        assert_eq!(scheduler.pop_due(200), Some((200, RTC)));
        assert_eq!(scheduler.next_deadline(), None);
    }

    #[test]
    fn scheduling_a_queued_key_again_moves_it() {
        let mut scheduler = Scheduler::new();
        scheduler.schedule(RTC, 100);
        scheduler.schedule(RTC, 300);

        assert_eq!(scheduler.pop_due(200), None);
        assert_eq!(scheduler.pop_due(300), Some((300, RTC)));
        assert_eq!(scheduler.pop_due(1_000), None);
    }

    #[test]
    fn a_cancelled_event_never_comes_due() {
        let mut scheduler = Scheduler::new();
        scheduler.schedule(RTC, 100);
        scheduler.cancel(RTC);

        assert!(!scheduler.is_scheduled(RTC));
        assert_eq!(scheduler.next_deadline(), None);
    }

    #[test]
    fn moving_an_alarm_again_and_again_keeps_the_queue_small() {
        let mut scheduler = Scheduler::new();
        for step in 0..10_000 {
            scheduler.schedule(RTC, 1_000_000 + step);
        }

        assert!(scheduler.queued_entries() <= 2 * COMPACT_ABOVE);
        assert_eq!(scheduler.pop_due(2_000_000), Some((1_009_999, RTC)));
    }

    #[test]
    fn events_due_together_leave_in_the_order_they_were_queued() {
        let mut scheduler = Scheduler::new();
        scheduler.schedule(FLASH, 50);
        scheduler.schedule(RTC, 50);

        assert_eq!(scheduler.pop_due(50), Some((50, FLASH)));
        assert_eq!(scheduler.pop_due(50), Some((50, RTC)));
    }
}
