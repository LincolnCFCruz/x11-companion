//! The driver's reliability logic against a scripted fake of the mouse.

mod common;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use attack_shark_x11::protocol::*;
use attack_shark_x11::transport::Transport;
use attack_shark_x11::{Device, Error, Timing};
use common::{BUTTONS, DPI, LIGHTING, POLLING, PROFILES, bytes};

#[derive(Clone, Copy, PartialEq)]
enum Ack {
    Ok,
    Nack,
    /// Applied, but the ACK is lost on the way back.
    Lost,
}

struct FakeMouse {
    reports: HashMap<u8, Vec<u8>>,
    unlocked: bool,
    /// Read requests to lose before one gets through.
    lose_requests: u32,
    /// Unlocked reads that come back garbled.
    garble_reads: u32,
    ack: Ack,
    events: VecDeque<Vec<u8>>,
    read_requests: u32,
}

impl FakeMouse {
    fn new() -> Self {
        let reports = [PROFILES, DPI, LIGHTING, POLLING, BUTTONS].map(bytes).map(|r| (r[0], r)).into();
        Self {
            reports,
            unlocked: false,
            lose_requests: 0,
            garble_reads: 0,
            ack: Ack::Ok,
            events: VecDeque::new(),
            read_requests: 0,
        }
    }
}

/// Lets the test inspect the fake after handing it to a `Device`.
#[derive(Clone)]
struct Shared(Arc<Mutex<FakeMouse>>);

impl Transport for Shared {
    fn send_feature(&mut self, data: &[u8]) -> Result<(), Error> {
        let mut mouse = self.0.lock().unwrap();
        if data[0] == READ_REQUEST {
            mouse.read_requests += 1;
            if mouse.lose_requests > 0 {
                mouse.lose_requests -= 1;
            } else {
                mouse.unlocked = true;
            }
            return Ok(());
        }
        let status = if mouse.ack == Ack::Nack { 1 } else { 0 };
        if mouse.ack != Ack::Nack {
            mouse.reports.insert(data[0], data.to_vec());
        }
        if mouse.ack != Ack::Lost {
            mouse.events.push_back(vec![0x03, 0x55, 0x50, status, data[0]]);
        }
        Ok(())
    }

    fn get_feature(&mut self, report_id: u8) -> Result<Vec<u8>, Error> {
        let mut mouse = self.0.lock().unwrap();
        let mut reply = if report_id == READ_REQUEST {
            vec![READ_REQUEST, u8::from(mouse.unlocked)]
        } else if !mouse.unlocked || mouse.garble_reads > 0 {
            mouse.garble_reads = mouse.garble_reads.saturating_sub(u32::from(mouse.unlocked));
            mouse.unlocked = false;
            vec![report_id, 0x5A, 0x5A, 0x5A]
        } else {
            mouse.unlocked = false;
            mouse.reports[&report_id].clone()
        };
        reply.resize(64, 0);
        Ok(reply)
    }

    fn read_event(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, Error> {
        let event = self.0.lock().unwrap().events.pop_front();
        if event.is_none() {
            std::thread::sleep(timeout.min(Duration::from_millis(2)));
        }
        Ok(event)
    }
}

fn device() -> (Device<Shared>, Arc<Mutex<FakeMouse>>) {
    let fake = Arc::new(Mutex::new(FakeMouse::new()));
    let mut device = Device::new(Shared(fake.clone()), PRODUCT_ID_WIRELESS);
    device.timing = Timing {
        grant_timeout: Duration::from_millis(20),
        grant_poll: Duration::from_millis(1),
        read_timeout: Duration::from_millis(300),
        corrupt_backoff: Duration::from_millis(1),
        ack_timeout: Duration::from_millis(30),
        write_attempts: 3,
    };
    (device, fake)
}

#[test]
fn reads_the_active_profile() {
    let (mut device, fake) = device();
    let report: DpiReport = device.read(None).unwrap();
    assert_eq!(report.data(), bytes(DPI));
    assert_eq!(fake.lock().unwrap().read_requests, 2); // the profile report, then the DPI report
    assert_eq!(device.connection(), "2.4 GHz dongle");
}

#[test]
fn a_lost_read_request_is_sent_again() {
    let (mut device, fake) = device();
    fake.lock().unwrap().lose_requests = 3;
    let report: PollingRateReport = device.read(Some(1)).unwrap();
    assert_eq!(report.rate_hz(), Some(1000));
    assert_eq!(fake.lock().unwrap().read_requests, 4);
}

#[test]
fn a_silent_mouse_is_reported_as_not_answering() {
    let (mut device, fake) = device();
    fake.lock().unwrap().lose_requests = u32::MAX;
    let error = device.read::<PollingRateReport>(Some(1)).unwrap_err();
    assert!(matches!(error, Error::NoResponse(_)), "{error}");
}

#[test]
fn a_garbled_read_is_retried() {
    let (mut device, fake) = device();
    fake.lock().unwrap().garble_reads = 2;
    let report: LightingReport = device.read(Some(1)).unwrap();
    assert_eq!(report.debounce_ms(), 8);
}

#[test]
fn persistent_garbage_is_an_error() {
    let (mut device, fake) = device();
    fake.lock().unwrap().garble_reads = u32::MAX;
    let error = device.read::<LightingReport>(Some(1)).unwrap_err();
    assert!(error.to_string().starts_with("corrupted LightingReport read"), "{error}");
}

#[test]
fn a_write_is_confirmed_by_its_ack() {
    let (mut device, fake) = device();
    let mut report: PollingRateReport = device.read(Some(1)).unwrap();
    report.set_rate_hz(500).unwrap();
    device.write(&report).unwrap();
    assert_eq!(fake.lock().unwrap().reports[&0x06], report.to_bytes());
}

#[test]
fn a_nack_is_an_error() {
    let (mut device, fake) = device();
    fake.lock().unwrap().ack = Ack::Nack;
    let report: PollingRateReport = device.read(Some(1)).unwrap();
    let error = device.write(&report).unwrap_err();
    assert!(error.to_string().starts_with("the mouse rejected report 0x06"), "{error}");
}

#[test]
fn a_lost_ack_is_settled_by_reading_back() {
    let (mut device, fake) = device();
    fake.lock().unwrap().ack = Ack::Lost;
    let mut report: DpiReport = device.read(Some(1)).unwrap();
    report.set_stage_dpi(5, 4800).unwrap();
    device.write(&report).unwrap();
    assert_eq!(fake.lock().unwrap().reports[&0x04], report.to_bytes());
}

#[test]
fn a_stale_ack_cannot_confirm_a_new_write() {
    let (mut device, fake) = device();
    {
        let mut mouse = fake.lock().unwrap();
        mouse.events.push_back(vec![0x03, 0x55, 0x50, 0x00, 0x06]); // left over from earlier
        mouse.ack = Ack::Nack;
    }
    let report: PollingRateReport = device.read(Some(1)).unwrap();
    assert!(device.write(&report).is_err());
}

#[test]
fn events_update_the_battery_and_reach_the_hook() {
    let (mut device, fake) = device();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    device.set_on_event(move |event| sink.lock().unwrap().push(*event));
    fake.lock().unwrap().events.extend([bytes("0355400139"), bytes("0355100300"), bytes("0355ffff00")]);
    let events = device.pending_events().unwrap();
    assert_eq!(events.len(), 2); // the unknown packet is skipped
    assert_eq!(device.battery(), Some(Battery { level: 0x39, state_code: 1 }));
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[test]
fn read_battery_waits_for_a_report() {
    let (mut device, fake) = device();
    fake.lock().unwrap().events.extend([bytes("0355100200"), bytes("0355400364")]);
    let battery = device.read_battery(Duration::from_millis(100)).unwrap();
    assert_eq!(battery.to_string(), "100% (charging)");
    assert!(matches!(device.read_battery(Duration::from_millis(20)), Err(Error::NoResponse(_))));
}
