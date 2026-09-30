use std::time::Duration;

use attack_shark_x11::battery::{LowBatteryAlerts, summary};
use attack_shark_x11::protocol::Battery;

#[test]
fn alerts_warn_once_at_each_threshold() {
    let mut alerts = LowBatteryAlerts::default();
    assert_eq!(alerts.update(40, true), None);
    assert_eq!(alerts.update(16, true), None);
    assert!(alerts.update(15, true).unwrap().contains("15%"));
    assert_eq!(alerts.update(12, true), None);
    assert!(alerts.update(5, true).unwrap().contains("Charge it now"));
    assert_eq!(alerts.update(3, true), None);
}

#[test]
fn starting_low_warns_only_the_urgent_one() {
    let mut alerts = LowBatteryAlerts::default();
    assert!(alerts.update(4, true).unwrap().contains("4%"));
    assert_eq!(alerts.update(4, true), None);
}

#[test]
fn alerts_rearm_after_charging_or_recovering() {
    let mut alerts = LowBatteryAlerts::default();
    assert!(alerts.update(14, true).is_some());
    assert_eq!(alerts.update(14, false), None); // charging clears the warnings
    assert!(alerts.update(14, true).is_some());
    assert_eq!(alerts.update(20, true), None); // climbed back above 15 + 5
    assert!(alerts.update(15, true).is_some());
}

#[test]
fn charging_never_warns() {
    assert_eq!(LowBatteryAlerts::default().update(3, false), None);
}

#[test]
fn summaries() {
    let fresh = Duration::from_secs(1);
    let reading = |level, state_code| Battery { level, state_code };
    assert_eq!(summary("X", false, None), "X · dongle not found");
    assert_eq!(summary("X", true, None), "X · mouse off or out of range");
    assert_eq!(summary("X", true, Some((reading(36, 1), Duration::from_secs(60)))), "X · mouse off or out of range");
    assert_eq!(summary("X", true, Some((reading(36, 1), fresh))), "X · 36% · discharging");
    assert_eq!(summary("X", true, Some((reading(100, 2), fresh))), "X · 100% · fully charged");
    assert_eq!(summary("X", true, Some((reading(50, 9), fresh))), "X · 50%");
}
