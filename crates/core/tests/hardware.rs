//! Against a real mouse: `cargo test -p attack-shark-x11 -- --ignored`. Read-only.

use std::time::Duration;

use attack_shark_x11::Device;
use attack_shark_x11::protocol::*;

#[test]
#[ignore = "needs an Attack Shark X11 plugged in"]
fn reads_every_report_and_the_battery() {
    let mut mouse = Device::open().expect("mouse connected");
    let profiles: ProfileReport = mouse.read(None).unwrap();
    let profile = Some(profiles.active_profile());
    assert!(profiles.checksum_ok());
    assert!(mouse.read::<DpiReport>(profile).unwrap().checksum_ok());
    assert!(mouse.read::<LightingReport>(profile).unwrap().checksum_ok());
    assert!(mouse.read::<PollingRateReport>(profile).unwrap().rate_hz().is_some());
    assert!(mouse.read::<ButtonsReport>(profile).unwrap().has_left_click());
    let battery = mouse.read_battery(Duration::from_secs(5)).unwrap();
    assert!(battery.level <= 100, "{battery}");
}
