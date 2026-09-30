//! Battery helpers for the tray: low-battery alerts and a one-line summary.

use std::time::Duration;

use crate::protocol::{Battery, BatteryState};

/// Warn at this level while discharging.
pub const LOW: u8 = 15;
/// And warn again at this one.
pub const CRITICAL: u8 = 5;
/// A warning comes back once the level has climbed this far above its threshold.
const REARM_MARGIN: u8 = 5;
/// Without a battery report for this long, the mouse is off or out of range.
pub const STALE_AFTER: Duration = Duration::from_secs(10);

/// Decides when to warn: once at 15% and once at 5% per discharge.
#[derive(Debug, Default)]
pub struct LowBatteryAlerts {
    warned: Vec<u8>,
}

impl LowBatteryAlerts {
    /// Feed a battery reading; returns the message to show, if any.
    pub fn update(&mut self, level: u8, discharging: bool) -> Option<String> {
        if !discharging {
            self.warned.clear();
            return None;
        }
        self.warned.retain(|threshold| level < threshold + REARM_MARGIN);
        for threshold in [CRITICAL, LOW] {
            if level <= threshold && !self.warned.contains(&threshold) {
                // A critical warning also covers the low one.
                for covered in [CRITICAL, LOW] {
                    if covered >= threshold && !self.warned.contains(&covered) {
                        self.warned.push(covered);
                    }
                }
                let advice = if threshold == CRITICAL { "Charge it now." } else { "Charge it soon." };
                return Some(format!("Mouse battery at {level}%. {advice}"));
            }
        }
        None
    }
}

/// One-line status such as `Attack Shark X11 · 57% · discharging`. `battery` is the last report and
/// how long ago it arrived.
pub fn summary(title: &str, connected: bool, battery: Option<(Battery, Duration)>) -> String {
    if !connected {
        return format!("{title} · dongle not found");
    }
    match battery {
        Some((battery, age)) if age <= STALE_AFTER => {
            let state = match battery.state() {
                Some(BatteryState::Discharging) => " · discharging",
                Some(BatteryState::Charging) => " · charging",
                Some(BatteryState::Full) => " · fully charged",
                None => "",
            };
            format!("{title} · {}%{state}", battery.level)
        }
        _ => format!("{title} · mouse off or out of range"),
    }
}
