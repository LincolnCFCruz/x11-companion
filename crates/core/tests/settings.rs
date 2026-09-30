mod common;

use std::collections::{BTreeMap, HashMap};

use attack_shark_x11::protocol::*;
use attack_shark_x11::settings::*;
use attack_shark_x11::{Error, Invalid};
use common::{BUTTONS, DPI, LIGHTING, POLLING, PROFILES, bytes};

/// Reports keyed by (report ID, profile); profile 2 was never configured.
struct FakeMouse {
    reports: HashMap<(u8, u8), Vec<u8>>,
    writes: Vec<Vec<u8>>,
}

impl FakeMouse {
    fn new() -> Self {
        let mut unset_buttons = vec![0x08, 0x3B, 0x02];
        unset_buttons.resize(0x3B, 0);
        let reports = HashMap::from([
            ((0x0C, 0), bytes(PROFILES)),
            ((0x04, 1), bytes(DPI)),
            ((0x05, 1), bytes(LIGHTING)),
            ((0x06, 1), bytes(POLLING)),
            ((0x08, 1), bytes(BUTTONS)),
            ((0x08, 2), unset_buttons),
        ]);
        Self { reports, writes: Vec::new() }
    }

    fn active(&self) -> u8 {
        self.reports[&(0x0C, 0)][2]
    }
}

impl Mouse for FakeMouse {
    fn read<R: Report>(&mut self, profile: Option<u8>) -> Result<R, Error> {
        let key = if R::PROFILE_SCOPED { profile.unwrap_or(self.active()) } else { 0 };
        Ok(R::from_bytes(&self.reports[&(R::ID, key)])?)
    }

    fn write<R: Report>(&mut self, report: &R) -> Result<(), Error> {
        let data = report.to_bytes();
        assert!(R::from_bytes(&data)?.checksum_ok());
        self.reports.insert((R::ID, report.profile().unwrap_or(0)), data.clone());
        self.writes.push(data);
        Ok(())
    }
}

#[test]
fn read_state_matches_the_old_json() {
    let state = read_state(&mut FakeMouse::new()).unwrap();
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(json["profile"], serde_json::json!({"active": 1, "count": 5}));
    assert_eq!(json["polling_rate"], 1000);
    assert_eq!(json["dpi"]["stages"], serde_json::json!([800, 1600, 2400, 3200, 5000, 22000]));
    assert_eq!(json["dpi"]["colors"][1], "#00ff00");
    assert_eq!(
        json["lighting"],
        serde_json::json!({"mode": "off", "color": "#00ff00", "speed": 3, "brightness": 8,
                           "sleep": 0.5, "deep_sleep": 10, "debounce": 8})
    );
    assert_eq!(json["buttons"]["dpi"], "dpi-cycle");
    assert_eq!(json["buttons"]["wheel-down"], "scroll-down");
}

#[test]
fn apply_dpi_changes_and_writes_once() {
    let mut mouse = FakeMouse::new();
    let patch: DpiPatch = serde_json::from_value(serde_json::json!(
        {"stages": [400, 800], "active": 1, "colors": ["#010203"], "ripple": true}
    ))
    .unwrap();
    let view = apply_dpi(&mut mouse, None, &patch).unwrap();
    assert_eq!(view.stages[..3], [400, 800, 2400]);
    assert_eq!(view.active, 1);
    assert_eq!(view.colors[0], "#010203");
    assert!(view.ripple);
    assert_eq!(mouse.writes.len(), 1);
}

#[test]
fn unchanged_values_write_nothing() {
    let mut mouse = FakeMouse::new();
    apply_dpi(&mut mouse, None, &DpiPatch { active: Some(2), ..Default::default() }).unwrap();
    apply_lighting(&mut mouse, None, &LightingPatch { debounce: Some(8), ..Default::default() }).unwrap();
    apply_polling_rate(&mut mouse, None, Some(1000)).unwrap();
    assert!(mouse.writes.is_empty());
}

#[test]
fn stage_count_applies_before_active() {
    let mut mouse = FakeMouse::new();
    let view =
        apply_dpi(&mut mouse, None, &DpiPatch { count: Some(3), active: Some(3), ..Default::default() }).unwrap();
    assert_eq!((view.count, view.active), (3, 3));
    let error = apply_dpi(&mut mouse, None, &DpiPatch { active: Some(5), ..Default::default() }).unwrap_err();
    assert!(matches!(error, Error::Invalid(_)), "{error}");
}

#[test]
fn apply_lighting_fields() {
    let mut mouse = FakeMouse::new();
    let patch = LightingPatch {
        mode: Some("breathing-dpi".into()),
        speed: Some(5),
        sleep: Some(2.5),
        deep_sleep: Some(30),
        ..Default::default()
    };
    let view = apply_lighting(&mut mouse, None, &patch).unwrap();
    assert_eq!((view.mode, view.speed, view.sleep, view.deep_sleep), (Some("breathing-dpi"), 5, 2.5, 30));
    let error = apply_lighting(&mut mouse, None, &LightingPatch { mode: Some("disco".into()), ..Default::default() });
    assert!(matches!(error, Err(Error::Invalid(Invalid(message))) if message.contains("disco")));
}

#[test]
fn apply_buttons_and_reset() {
    let mut mouse = FakeMouse::new();
    let bindings = BTreeMap::from([("forward".into(), "key:ctrl+c".into()), ("back".into(), "play-pause".into())]);
    let view = apply_buttons(&mut mouse, None, &ButtonsPatch { reset: false, bindings }, false).unwrap();
    assert_eq!((view["forward"].as_str(), view["back"].as_str()), ("key:ctrl+c", "play-pause"));
    let view = apply_buttons(&mut mouse, None, &ButtonsPatch { reset: true, ..Default::default() }, false).unwrap();
    assert_eq!(view["forward"], "forward");
}

#[test]
fn buttons_keep_a_left_click_unless_forced() {
    let mut mouse = FakeMouse::new();
    let disable_left =
        ButtonsPatch { bindings: BTreeMap::from([("left".into(), "disabled".into())]), ..Default::default() };
    let error = apply_buttons(&mut mouse, None, &disable_left, false).unwrap_err();
    assert!(matches!(error, Error::Guard(ref message) if message == NO_LEFT_CLICK));
    assert!(mouse.writes.is_empty());
    apply_buttons(&mut mouse, None, &disable_left, true).unwrap();
    assert_eq!(mouse.writes.len(), 1);

    let moved = ButtonsPatch {
        bindings: BTreeMap::from([("left".into(), "back".into()), ("right".into(), "left-click".into())]),
        ..Default::default()
    };
    apply_buttons(&mut FakeMouse::new(), None, &moved, false).unwrap(); // moved, not removed
}

#[test]
fn unknown_button_is_rejected() {
    let patch = ButtonsPatch { bindings: BTreeMap::from([("thumb".into(), "back".into())]), ..Default::default() };
    assert!(matches!(apply_buttons(&mut FakeMouse::new(), None, &patch, false), Err(Error::Invalid(_))));
}

#[test]
fn profile_switch_refuses_an_unset_profile() {
    let mut mouse = FakeMouse::new();
    let error = apply_profile(&mut mouse, Some(2), false).unwrap_err();
    assert!(matches!(error, Error::Guard(ref message) if message.contains("looks unset")));
    assert!(mouse.writes.is_empty());
    assert!(matches!(apply_profile(&mut mouse, Some(9), false), Err(Error::Invalid(_))));
    assert_eq!(apply_profile(&mut mouse, None, false).unwrap(), ProfileView { active: 1, count: 5 });
}

#[test]
fn meta_lists() {
    let meta = meta();
    assert_eq!(meta.polling_rates, [125, 250, 500, 1000]);
    assert_eq!(meta.light_modes[5], "static-dpi");
    assert_eq!(meta.buttons.len(), 8);
    assert!(meta.actions.contains(&"dpi-cycle") && !meta.actions.contains(&"profile-cycle"));
    assert_eq!(&meta.keys[..3], ["a", "b", "c"]);
    assert!(meta.keys.contains(&"f12") && meta.keys.contains(&"pagedown"));
}
