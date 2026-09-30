//! Driver for the Attack Shark X11 mouse: its configuration protocol, a reliable HID transport over
//! the 2.4 GHz dongle or the USB cable, and read-modify-write settings on top.
//!
//! ```no_run
//! use attack_shark_x11::{Device, protocol::DpiReport};
//!
//! let mut mouse = Device::open()?;
//! println!("{}", mouse.read_battery(std::time::Duration::from_secs(5))?);
//! let mut dpi: DpiReport = mouse.read(None)?; // the active profile
//! dpi.set_stage_dpi(1, 400)?;
//! mouse.write(&dpi)?; // confirmed by the mouse
//! # Ok::<(), attack_shark_x11::Error>(())
//! ```

pub mod battery;
pub mod device;
pub mod protocol;
pub mod settings;
pub mod transport;

pub use device::{Device, Timing};
pub use protocol::Invalid;

/// Anything that can go wrong talking to the mouse.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Attack Shark X11 not found: plug in the 2.4 GHz dongle (1d57:fa60) or the USB cable (1d57:fa55)")]
    NotFound,
    /// The mouse didn't answer: asleep, off or out of range. The dongle is still there.
    #[error("{0}")]
    NoResponse(String),
    /// Anything else that went wrong on the wire.
    #[error("{0}")]
    Device(String),
    /// A value the mouse doesn't accept.
    #[error(transparent)]
    Invalid(#[from] Invalid),
    /// A change refused because it would leave the mouse hard to use.
    #[error("{0}")]
    Guard(String),
}
