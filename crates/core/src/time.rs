// SPDX-License-Identifier: GPL-3.0-only
//! Emulated time, counted in cycles of the 64 MHz CPU clock since power-on.

pub type Cycle = u64;

pub const CPU_HERTZ: u64 = 64_000_000;

/// The 32.768 kHz low-frequency clock that the RTCs, the watchdog and the touch timings run from.
pub const LOW_FREQUENCY_HERTZ: u64 = 32_768;

const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

pub fn cycles_from_nanoseconds(nanoseconds: u64) -> Cycle {
    (nanoseconds as u128 * CPU_HERTZ as u128 / NANOSECONDS_PER_SECOND as u128) as Cycle
}

pub fn cycles_from_microseconds(microseconds: u64) -> Cycle {
    microseconds * (CPU_HERTZ / 1_000_000)
}

pub fn cycles_from_milliseconds(milliseconds: u64) -> Cycle {
    milliseconds * (CPU_HERTZ / 1_000)
}

pub fn nanoseconds_from_cycles(cycles: Cycle) -> u64 {
    (cycles as u128 * NANOSECONDS_PER_SECOND as u128 / CPU_HERTZ as u128) as u64
}

/// The number of whole ticks of a clock of `hertz` that have passed by `cycle`.
pub fn ticks_at(cycle: Cycle, hertz: u64) -> u64 {
    (cycle as u128 * hertz as u128 / CPU_HERTZ as u128) as u64
}

/// The first cycle at which `ticks` whole ticks of a clock of `hertz` have passed.
pub fn cycle_of_tick(ticks: u64, hertz: u64) -> Cycle {
    (ticks as u128 * CPU_HERTZ as u128).div_ceil(hertz as u128) as Cycle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_cycles_and_wall_units() {
        assert_eq!(cycles_from_milliseconds(1), 64_000);
        assert_eq!(cycles_from_microseconds(3), 192);
        assert_eq!(cycles_from_nanoseconds(1_000), 64);
        assert_eq!(nanoseconds_from_cycles(64), 1_000);
    }

    #[test]
    fn a_tick_starts_on_the_first_cycle_that_reaches_it() {
        let hertz = LOW_FREQUENCY_HERTZ;
        for tick in [1, 2, 7, 32_767, 32_768, 1_000_000] {
            let cycle = cycle_of_tick(tick, hertz);
            assert_eq!(ticks_at(cycle, hertz), tick);
            assert_eq!(ticks_at(cycle - 1, hertz), tick - 1);
        }
    }
}
