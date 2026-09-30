//! Raw access to the mouse's configuration interface.
//!
//! Interface 2 carries the protocol as two top-level HID collections: one takes feature reports
//! (writes, plus the read handshake), the other emits 5-byte event packets. On Windows each
//! collection is its own device path; on Linux (one hidraw node) and macOS both share one device.

use std::ffi::CString;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use hidapi::{HidApi, HidDevice};

use crate::Error;
use crate::protocol::{
    COMMAND_USAGE_PAGE, CONFIG_INTERFACE, EVENT_USAGE_PAGE, PRODUCT_ID_WIRED, PRODUCT_ID_WIRELESS, VENDOR_ID,
};

/// Windows sizes every feature report of the command collection to 64 bytes.
const FEATURE_BUFFER: usize = 64;

/// Linux lets only root open the mouse until a udev rule grants access.
const OPEN_HINT: &str =
    if cfg!(target_os = "linux") { " (if access was denied, install the udev rule from the README)" } else { "" };

/// What the driver needs from the device. [`HidTransport`] is the real one; tests use fakes.
pub trait Transport {
    /// Send a feature report; byte 0 is the report ID.
    fn send_feature(&mut self, data: &[u8]) -> Result<(), Error>;
    /// Get a feature report; the result starts with the report ID.
    fn get_feature(&mut self, report_id: u8) -> Result<Vec<u8>, Error>;
    /// The next event packet, waiting up to `timeout` (zero: don't wait).
    fn read_event(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, Error>;
}

/// The process-wide hidapi context.
fn hid_api() -> Result<MutexGuard<'static, HidApi>, Error> {
    static API: OnceLock<Mutex<HidApi>> = OnceLock::new();
    if API.get().is_none() {
        let api = HidApi::new().map_err(|e| Error::Device(format!("couldn't start HID access: {e}")))?;
        let _ = API.set(Mutex::new(api));
    }
    Ok(API.get().expect("initialized above").lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

#[derive(Debug)]
pub struct HidTransport {
    command: HidDevice,
    /// `None` when the events share the command device (Linux hidraw).
    events: Option<HidDevice>,
}

impl HidTransport {
    /// Open the first X11 found: the dongle, else the cable. Returns the transport and product ID.
    pub fn open() -> Result<(Self, u16), Error> {
        let mut api = hid_api()?;
        api.refresh_devices().map_err(|e| Error::Device(format!("couldn't list HID devices: {e}")))?;
        for product_id in [PRODUCT_ID_WIRELESS, PRODUCT_ID_WIRED] {
            let (mut command, mut events): (Option<CString>, Option<CString>) = (None, None);
            for info in api.device_list() {
                if info.vendor_id() != VENDOR_ID
                    || info.product_id() != product_id
                    || info.interface_number() != CONFIG_INTERFACE
                {
                    continue;
                }
                match info.usage_page() {
                    COMMAND_USAGE_PAGE => command = Some(info.path().to_owned()),
                    EVENT_USAGE_PAGE => events = Some(info.path().to_owned()),
                    _ => {}
                }
            }
            let (Some(command_path), Some(event_path)) = (command, events) else { continue };
            let open = |path: &CString| {
                api.open_path(path).map_err(|e| Error::Device(format!("could not open the mouse: {e}{OPEN_HINT}")))
            };
            let command = open(&command_path)?;
            let events = if event_path == command_path { None } else { Some(open(&event_path)?) };
            return Ok((Self { command, events }, product_id));
        }
        Err(Error::NotFound)
    }
}

impl Transport for HidTransport {
    fn send_feature(&mut self, data: &[u8]) -> Result<(), Error> {
        self.command.send_feature_report(data).map_err(|e| Error::Device(format!("talking to the mouse failed: {e}")))
    }

    fn get_feature(&mut self, report_id: u8) -> Result<Vec<u8>, Error> {
        let mut buffer = [0u8; FEATURE_BUFFER];
        buffer[0] = report_id;
        self.command
            .get_feature_report(&mut buffer)
            .map_err(|e| Error::Device(format!("reading from the mouse failed: {e}")))?;
        Ok(buffer.to_vec())
    }

    fn read_event(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, Error> {
        let device = self.events.as_ref().unwrap_or(&self.command);
        let mut buffer = [0u8; 64];
        let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let read = device
            .read_timeout(&mut buffer, millis)
            .map_err(|e| Error::Device(format!("lost connection to the mouse: {e}")))?;
        Ok((read > 0).then(|| buffer[..read].to_vec()))
    }
}
