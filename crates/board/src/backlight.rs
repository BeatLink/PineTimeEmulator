// SPDX-License-Identifier: GPL-3.0-only
//! The screen's backlight: three active-low enable lines for the low, mid and high levels.

/// Which backlight enable line a pin drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BacklightLine {
    Low = 0,
    Mid = 1,
    High = 2,
}

pub const FULL_LEVEL: u8 = 3;

const PERCENT_BY_LEVEL: [u32; 4] = [0, 40, 70, 100];

/// The backlight level, 0 off to 3 full; any line that changes sets it from all three lines.
pub struct Backlight {
    line_low: [bool; 3],
    level: u8,
}

impl Default for Backlight {
    fn default() -> Backlight {
        Backlight::new()
    }
}

impl Backlight {
    pub fn new() -> Backlight {
        Backlight {
            line_low: [false; 3],
            level: FULL_LEVEL,
        }
    }

    pub fn set_line(&mut self, line: BacklightLine, pin_high: bool) {
        self.line_low[line as usize] = !pin_high;
        self.level = self.level_from_lines();
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    /// Overrides the level until a line next changes.
    pub fn set_level(&mut self, level: u8) {
        self.level = level.min(FULL_LEVEL);
    }

    /// Scales an 8-bit colour channel by the backlight's brightness.
    pub fn dim(&self, channel: u32) -> u32 {
        channel * PERCENT_BY_LEVEL[self.level as usize] / 100
    }

    fn level_from_lines(&self) -> u8 {
        (1..=FULL_LEVEL)
            .rev()
            .find(|level| self.line_low[*level as usize - 1])
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_full_brightness_before_any_line_changes() {
        assert_eq!(Backlight::new().level(), FULL_LEVEL);
    }

    #[test]
    fn the_highest_low_line_sets_the_level() {
        let mut backlight = Backlight::new();
        backlight.set_line(BacklightLine::Low, false);
        assert_eq!(backlight.level(), 1);
        backlight.set_line(BacklightLine::Mid, false);
        assert_eq!(backlight.level(), 2);
        backlight.set_line(BacklightLine::High, false);
        assert_eq!(backlight.level(), 3);
        backlight.set_line(BacklightLine::High, true);
        backlight.set_line(BacklightLine::Mid, true);
        assert_eq!(backlight.level(), 1);
        backlight.set_line(BacklightLine::Low, true);
        assert_eq!(backlight.level(), 0);
    }

    #[test]
    fn dims_a_channel_by_level() {
        let mut backlight = Backlight::new();
        assert_eq!(backlight.dim(255), 255);
        backlight.set_level(1);
        assert_eq!(backlight.dim(255), 102);
        backlight.set_level(9);
        assert_eq!(backlight.level(), 3);
        backlight.set_level(0);
        assert_eq!(backlight.dim(255), 0);
    }
}
