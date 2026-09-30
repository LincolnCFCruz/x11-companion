mod common;

use attack_shark_x11::protocol::*;
use common::{BUTTONS, DPI, LIGHTING, POLLING, PROFILES, bytes};

fn parsed<R: Report>(hex: &str) -> R {
    R::from_bytes(&bytes(hex)).unwrap()
}

fn round_trips<R: Report>(hex: &str) {
    let report: R = parsed(hex);
    assert!(report.checksum_ok(), "{}", R::NAME);
    assert_eq!(report.to_bytes(), bytes(hex), "{}", R::NAME);
}

#[test]
fn device_reports_have_valid_checksums_and_round_trip() {
    round_trips::<DpiReport>(DPI);
    round_trips::<LightingReport>(LIGHTING);
    round_trips::<PollingRateReport>(POLLING);
    round_trips::<ProfileReport>(PROFILES);
    round_trips::<ButtonsReport>(BUTTONS);
}

#[test]
fn checksum_mismatch_is_detected() {
    let mut corrupted = bytes(DPI);
    corrupted[9] ^= 1;
    assert!(!DpiReport::from_bytes(&corrupted).unwrap().checksum_ok());
}

#[test]
fn wrong_report_id_or_length_is_rejected() {
    let mut lighting = bytes(LIGHTING);
    lighting.resize(64, 0);
    assert!(DpiReport::from_bytes(&lighting).is_err());
    assert!(DpiReport::from_bytes(&bytes(DPI)[..20]).is_err());
}

#[test]
fn dpi_report_decodes_factory_stages() {
    let report: DpiReport = parsed(DPI);
    assert_eq!(report.profile(), Some(1));
    assert_eq!(report.stages(), [800, 1600, 2400, 3200, 5000, 22000]);
    assert_eq!(report.active_stage(), 2);
    assert_eq!(report.stage_count(), 6);
    assert!(!report.angle_snap());
    assert!(!report.ripple_control());
    assert_eq!(report.stage_color(1).unwrap(), [0xFF, 0, 0]);
    assert_eq!(report.stage_color(6).unwrap(), [0xFF, 0, 0xFF]);
}

#[test]
fn official_software_dpi_packet() {
    // Sent by the official software: same stages, ripple control on, unused stage 7/8 codes zero.
    let sent = bytes(
        "04380100013f2020 1225384b75810000 0000000000010000 02 \
         ff0000 00ff00 0000ff ffff00 00ffff ff00ff ff4000 ffffff 02 0f68 00000000",
    );
    let mut factory = bytes(DPI);
    factory[14..16].copy_from_slice(&[0, 0]);
    let mut report = DpiReport::from_bytes(&factory).unwrap();
    report.set_ripple_control(true);
    assert_eq!(report.to_bytes(), sent);
}

#[test]
fn encode_dpi_matches_official_software() {
    for (dpi, expected) in [
        (50, (0x01, false, false)),
        (800, (0x12, false, false)),
        (10000, (0xEB, false, false)),
        (11000, (0x81, true, false)), // 5500 x2 via the high byte
        (12000, (0x8D, true, false)),
        (12100, (0x8E, false, true)), // 6050 x2 via the double mask
        (20000, (0xEB, false, true)),
        (22000, (0x81, true, true)), // 5500 x4
    ] {
        assert_eq!(encode_dpi(dpi).unwrap(), expected, "{dpi}");
        assert_eq!(decode_dpi(expected.0, expected.1, expected.2), dpi);
    }
}

#[test]
fn every_supported_dpi_round_trips() {
    let supported = (50..=22000).step_by(50).filter(|&d| d <= 10000 || (d % 100 == 0 && (d <= 20000 || d % 200 == 0)));
    for dpi in supported {
        let (code, high, double) = encode_dpi(dpi).unwrap();
        assert_eq!(decode_dpi(code, high, double), dpi);
    }
}

#[test]
fn unsupported_dpi_is_rejected() {
    for dpi in [0, 25, 825, 10050, 20100, 22200] {
        assert!(encode_dpi(dpi).is_err(), "{dpi}");
    }
}

#[test]
fn setting_dpi_updates_both_double_masks_and_high_byte() {
    let mut report: DpiReport = parsed(DPI);
    report.set_stage_dpi(6, 3200).unwrap();
    assert_eq!((report.data()[6], report.data()[7], report.data()[21]), (0, 0, 0));
    report.set_stage_dpi(2, 16000).unwrap();
    assert_eq!((report.data()[6], report.data()[7], report.data()[17]), (0x02, 0x02, 0));
    assert_eq!(report.stages(), [800, 16000, 2400, 3200, 5000, 3200]);
    assert!(DpiReport::from_bytes(&report.to_bytes()).unwrap().checksum_ok());
}

#[test]
fn stage_count_clamps_active_stage() {
    let mut report: DpiReport = parsed(DPI);
    report.set_active_stage(5).unwrap();
    report.set_stage_count(3).unwrap();
    assert_eq!(report.data()[5], 0b111);
    assert_eq!(report.active_stage(), 3);
    assert!(report.set_active_stage(4).is_err());
}

#[test]
fn setters_keep_unknown_nibbles() {
    let mut data = bytes(DPI);
    data[3] = 0x20; // lift-off distance nibble
    let mut report = DpiReport::from_bytes(&data).unwrap();
    report.set_angle_snap(true);
    assert_eq!(report.data()[3], 0x21);
}

#[test]
fn lighting_report_decodes_fields() {
    let report: LightingReport = parsed(LIGHTING);
    assert_eq!(report.mode(), Some(LightMode::Off));
    assert_eq!(report.speed(), 3);
    assert_eq!(report.brightness(), 8);
    assert_eq!(report.color(), [0, 0xFF, 0]);
    assert_eq!(report.sleep_minutes(), 0.5);
    assert_eq!(report.deep_sleep_minutes(), 10);
    assert_eq!(report.debounce_ms(), 8);
}

#[test]
fn lighting_checksum_matches_official_software() {
    // Official-software lighting packets (iago-fragnan/attack-shark-x11-linux).
    for packet in ["050f011001a8000000010600c00000", "050f012001a80000ff010601cf0000", "050f013001a80000ff010601df0000"]
    {
        assert!(parsed::<LightingReport>(packet).checksum_ok(), "{packet}");
    }
}

#[test]
fn lighting_setters() {
    let mut report: LightingReport = parsed(LIGHTING);
    report.set_mode(LightMode::Breathing);
    report.set_speed(5).unwrap();
    report.set_brightness(4).unwrap();
    report.set_color([1, 2, 3]);
    report.set_sleep_minutes(2.5).unwrap();
    report.set_deep_sleep_minutes(45).unwrap();
    report.set_debounce_ms(12).unwrap();
    let again = LightingReport::from_bytes(&report.to_bytes()).unwrap();
    assert_eq!(
        (again.mode(), again.speed(), again.brightness(), again.color()),
        (Some(LightMode::Breathing), 5, 4, [1, 2, 3])
    );
    assert_eq!((again.sleep_minutes(), again.deep_sleep_minutes(), again.debounce_ms()), (2.5, 45, 12));
    assert_eq!(again.data()[4], 0x21); // deep sleep high nibble (45 = 0x2d) | stored speed (6 - 5)
}

#[test]
fn lighting_rejects_out_of_range() {
    let mut report: LightingReport = parsed(LIGHTING);
    assert!(report.set_speed(0).is_err());
    assert!(report.set_brightness(9).is_err());
    assert!(report.set_sleep_minutes(0.75).is_err());
    assert!(report.set_deep_sleep_minutes(61).is_err());
    assert!(report.set_debounce_ms(7).is_err());
    assert_eq!(report.set_brightness(0).unwrap_err().to_string(), "brightness must be between 1 and 8, got 0");
}

#[test]
fn polling_rate() {
    for (hz, packet) in [(125, "06090108f700000000"), (1000, "06090101fe00000000")] {
        let mut report: PollingRateReport = parsed(POLLING);
        report.set_rate_hz(hz).unwrap();
        assert_eq!(report.to_bytes(), bytes(packet));
        assert_eq!(parsed::<PollingRateReport>(packet).rate_hz(), Some(hz));
    }
    assert!(parsed::<PollingRateReport>(POLLING).set_rate_hz(333).is_err());
}

#[test]
fn profile_report() {
    let mut report: ProfileReport = parsed(PROFILES);
    assert_eq!((report.active_profile(), report.profile_count()), (1, 5));
    assert_eq!(report.profile(), None); // not profile-scoped
    report.set_active_profile(3).unwrap();
    assert_eq!(report.to_bytes()[2..6], [3, 0xFC, 5, 0xFA]);
    assert!(report.set_active_profile(6).is_err());
}

#[test]
fn buttons_report_defaults() {
    let report: ButtonsReport = parsed(BUTTONS);
    for button in Button::ALL {
        assert_eq!(report.binding(button), button.default_action().into(), "{button:?}");
    }
    assert_eq!(report.binding(Button::Dpi).to_string(), "dpi-cycle");
    assert!(report.has_left_click());
}

#[test]
fn binding_parse_and_format() {
    let combo = Binding::parse("key:ctrl+shift+t").unwrap();
    assert_eq!(combo, Binding { action: Action::Key as u8, modifiers: 0x03, key: 0x17 });
    assert_eq!(combo.to_string(), "key:ctrl+shift+t");
    assert_eq!(Binding::parse("KEY:F5").unwrap(), Binding { action: Action::Key as u8, modifiers: 0, key: 0x3E });
    assert_eq!(Binding::parse("volume-up").unwrap(), Action::VolumeUp.into());
    for text in ["key:ctrl+nope", "key:hyper+a", "key:", "profile-cycle", "fire", "whatever"] {
        assert!(Binding::parse(text).is_err(), "{text}");
    }
    assert_eq!(Binding { action: 0x7F, modifiers: 0, key: 0 }.to_string(), "0x7f");
}

#[test]
fn set_binding_and_checksum() {
    let mut report: ButtonsReport = parsed(BUTTONS);
    report.set_binding(Button::Forward, Binding::parse("key:ctrl+c").unwrap());
    let data = report.to_bytes();
    assert_eq!(data[21..24], [0x11, 0x01, 0x06]);
    assert_eq!(u16::from_be_bytes([data[57], data[58]]), 0x34 - 0x06 + 0x11 + 0x01 + 0x06);
}

#[test]
fn parse_events() {
    let battery = |state_code, level| Some(Event::Battery(Battery { level, state_code }));
    assert_eq!(parse_event(&bytes("0355400127")), battery(1, 39));
    assert_eq!(
        parse_event(&bytes("0355400127")).map(|e| format!("{e:?}")),
        Some("Battery(Battery { level: 39, state_code: 1 })".into())
    );
    assert_eq!(Battery { level: 69, state_code: 3 }.to_string(), "69% (charging)");
    assert_eq!(Battery { level: 100, state_code: 2 }.to_string(), "100% (fully charged)");
    assert_eq!(parse_event(&bytes("0355500006")), Some(Event::Ack { report_id: 0x06, ok: true }));
    assert_eq!(parse_event(&bytes("0355500104")), Some(Event::Ack { report_id: 0x04, ok: false }));
    assert_eq!(parse_event(&bytes("0355100300")), Some(Event::DpiStage(3)));
    assert_eq!(parse_event(&bytes("0355800200")), Some(Event::Profile(3)));
    assert_eq!(parse_event(&bytes("0355ffff00")), None);
    assert_eq!(parse_event(&[0x01, 0x02]), None);
}

#[test]
fn read_request_bytes() {
    assert_eq!(read_request::<DpiReport>(1), [0xA0, 0x04, 0x38, 0, 1, 0, 0, 0]);
    assert_eq!(read_request::<ProfileReport>(0), [0xA0, 0x0C, 0x0A, 0, 0, 0, 0, 0]);
}

#[test]
fn colors() {
    assert_eq!(parse_color("#ff8800").unwrap(), [0xFF, 0x88, 0x00]);
    assert_eq!(parse_color(" 00ff00 ").unwrap(), [0, 0xFF, 0]);
    for text in ["zz0000", "#ff00", "ff00000", "##ff0000", "ffé000"] {
        assert!(parse_color(text).is_err(), "{text}");
    }
    assert_eq!(format_color([0x12, 0xAB, 0x00]), "#12ab00");
}
