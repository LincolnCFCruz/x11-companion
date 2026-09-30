//! The driver: reads and writes reports reliably over an unreliable radio link.
//!
//! Commands reach the mouse through the dongle's radio link. It drops about one read request in
//! 30, and now and then goes quiet for several seconds (6 s has been measured). So reads keep
//! re-sending their request until a deadline, and a write whose ACK went missing is settled by
//! reading the report back. A read that fails its checksum is retried after a pause.
//!
//! On Windows, a thread's pending reads are cancelled when it exits, so a `Device` should live on
//! one long-lived thread.

use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use crate::Error;
use crate::protocol::{
    Battery, Event, PRODUCT_ID_WIRELESS, ProfileReport, READ_REQUEST, Report, hex, parse_event, read_request,
};
use crate::transport::{HidTransport, Transport};

/// Timeouts and retry counts; tests shrink them.
#[derive(Debug, Clone)]
pub struct Timing {
    /// How long one read request may wait to be unlocked (typically 30-220 ms).
    pub grant_timeout: Duration,
    pub grant_poll: Duration,
    /// Total patience for a read, re-sending the request as needed.
    pub read_timeout: Duration,
    /// Pause after a read that failed its checksum.
    pub corrupt_backoff: Duration,
    pub ack_timeout: Duration,
    pub write_attempts: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            grant_timeout: Duration::from_millis(750),
            grant_poll: Duration::from_millis(20),
            read_timeout: Duration::from_secs(10),
            corrupt_backoff: Duration::from_millis(300),
            ack_timeout: Duration::from_millis(1500),
            write_attempts: 3,
        }
    }
}

type EventHook = Box<dyn FnMut(&Event) + Send>;

pub struct Device<T: Transport = HidTransport> {
    transport: T,
    product_id: u16,
    battery: Option<Battery>,
    on_event: Option<EventHook>,
    pub timing: Timing,
}

impl<T: Transport> fmt::Debug for Device<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Device").field("product_id", &self.product_id).field("battery", &self.battery).finish()
    }
}

impl Device<HidTransport> {
    /// Open the X11 behind the dongle, or on its cable.
    pub fn open() -> Result<Self, Error> {
        let (transport, product_id) = HidTransport::open()?;
        Ok(Self::new(transport, product_id))
    }
}

impl<T: Transport> Device<T> {
    pub fn new(transport: T, product_id: u16) -> Self {
        Self { transport, product_id, battery: None, on_event: None, timing: Timing::default() }
    }

    pub fn product_id(&self) -> u16 {
        self.product_id
    }

    pub fn connection(&self) -> &'static str {
        if self.product_id == PRODUCT_ID_WIRELESS { "2.4 GHz dongle" } else { "USB cable" }
    }

    /// The last battery report seen.
    pub fn battery(&self) -> Option<Battery> {
        self.battery
    }

    /// Called with every event read, including ones consumed while waiting for an ACK, so a
    /// listener never misses a battery or DPI-button event.
    pub fn set_on_event(&mut self, hook: impl FnMut(&Event) + Send + 'static) {
        self.on_event = Some(Box::new(hook));
    }

    // -- events -----------------------------------------------------------------------------

    fn parse(&mut self, packet: &[u8]) -> Option<Event> {
        let event = parse_event(packet)?;
        if let Event::Battery(battery) = event {
            self.battery = Some(battery);
        }
        if let Some(hook) = &mut self.on_event {
            hook(&event);
        }
        Some(event)
    }

    /// The next event, waiting up to `timeout` (zero: don't wait).
    pub fn poll_event(&mut self, timeout: Duration) -> Result<Option<Event>, Error> {
        let packet = self.transport.read_event(timeout)?;
        Ok(packet.and_then(|packet| self.parse(&packet)))
    }

    /// The events already queued, without waiting.
    pub fn pending_events(&mut self) -> Result<Vec<Event>, Error> {
        let mut events = Vec::new();
        while let Some(packet) = self.transport.read_event(Duration::ZERO)? {
            events.extend(self.parse(&packet));
        }
        Ok(events)
    }

    /// Wait for the first event `wanted` accepts, for `timeout` or forever.
    pub fn wait_for(
        &mut self,
        timeout: Option<Duration>,
        mut wanted: impl FnMut(&Event) -> bool,
    ) -> Result<Option<Event>, Error> {
        const SLICE: Duration = Duration::from_millis(250);
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            let slice = match deadline {
                None => SLICE,
                Some(deadline) => match deadline.saturating_duration_since(Instant::now()) {
                    left if left.is_zero() => return Ok(None),
                    left => left.min(SLICE),
                },
            };
            if let Some(event) = self.poll_event(slice)? {
                if wanted(&event) {
                    return Ok(Some(event));
                }
            }
        }
    }

    /// Wait for the next battery report (sent every ~2 s while the mouse is on).
    pub fn read_battery(&mut self, timeout: Duration) -> Result<Battery, Error> {
        match self.wait_for(Some(timeout), |event| matches!(event, Event::Battery(_)))? {
            Some(Event::Battery(battery)) => Ok(battery),
            _ => Err(Error::NoResponse(format!(
                "no battery report within {} s; the mouse may be asleep, so move it and try again",
                timeout.as_secs_f64()
            ))),
        }
    }

    // -- reports ------------------------------------------------------------------------------

    /// Read a report, for `profile` or else the active profile.
    pub fn read<R: Report>(&mut self, profile: Option<u8>) -> Result<R, Error> {
        let param = match (R::PROFILE_SCOPED, profile) {
            (false, _) => 0,
            (true, Some(profile)) => profile,
            (true, None) => self.read::<ProfileReport>(None)?.active_profile(),
        };
        let request = read_request::<R>(param);
        let mut error = Error::NoResponse("the mouse did not answer; move it to wake it up and try again".into());
        let deadline = Instant::now() + self.timing.read_timeout;
        while Instant::now() < deadline {
            let Some(data) = self.read_raw(&request)? else { continue };
            if let Ok(report) = R::from_bytes(&data) {
                if report.checksum_ok() {
                    return Ok(report);
                }
            }
            let shown = &data[..data.len().min(R::LEN)];
            error = Error::Device(format!("corrupted {} read: {}", R::NAME, hex(shown)));
            thread::sleep(self.timing.corrupt_backoff);
        }
        Err(error)
    }

    /// One read handshake; `None` if the mouse didn't unlock the read in time. The 0xA0 request
    /// unlocks exactly one GET_REPORT, and GET_REPORT 0xA0 answers `a0 01` once it is unlocked.
    fn read_raw(&mut self, request: &[u8; 8]) -> Result<Option<Vec<u8>>, Error> {
        self.transport.send_feature(request)?;
        let deadline = Instant::now() + self.timing.grant_timeout;
        while Instant::now() < deadline {
            thread::sleep(self.timing.grant_poll);
            let grant = self.transport.get_feature(READ_REQUEST)?;
            if grant.get(1) == Some(&0x01) {
                return self.transport.get_feature(request[1]).map(Some);
            }
        }
        Ok(None)
    }

    /// Send a report and confirm the mouse applied it.
    pub fn write<R: Report>(&mut self, report: &R) -> Result<(), Error> {
        let data = report.to_bytes();
        for _ in 0..self.timing.write_attempts {
            self.pending_events()?; // consume queued events so an old ACK can't match
            self.transport.send_feature(&data)?;
            let ack = self.wait_for(
                Some(self.timing.ack_timeout),
                |event| matches!(event, Event::Ack { report_id, .. } if *report_id == data[0]),
            )?;
            if let Some(Event::Ack { ok, .. }) = ack {
                if !ok {
                    return Err(Error::Device(format!("the mouse rejected report 0x{:02x}: {}", data[0], hex(&data))));
                }
                return Ok(());
            }
            // No ACK: read the report back. A NoResponse error propagates: re-sending wouldn't help.
            if self.read::<R>(report.profile())?.data() == data.as_slice() {
                return Ok(());
            }
        }
        Err(Error::Device(format!("the mouse did not apply report 0x{:02x}: {}", data[0], hex(&data))))
    }
}
