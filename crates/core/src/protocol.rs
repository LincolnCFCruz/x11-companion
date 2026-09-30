//! Wire format of the Attack Shark X11 configuration protocol.
//!
//! Pure data conversion with no I/O, so it can be tested against packets captured from a real
//! mouse. Each report wraps the raw bytes read from the device and is edited in place: bytes nobody
//! has decoded yet are carried through unchanged when a report is written back.
//!
//! Offsets are indexes into the report including its leading report-ID byte. `docs/PROTOCOL.md`
//! has the full reference.

use std::fmt;
use std::sync::OnceLock;

pub const VENDOR_ID: u16 = 0x1D57;
/// The 2.4 GHz dongle.
pub const PRODUCT_ID_WIRELESS: u16 = 0xFA60;
/// The mouse on its USB-C cable.
pub const PRODUCT_ID_WIRED: u16 = 0xFA55;

pub const CONFIG_INTERFACE: i32 = 2;
/// Top-level collection that takes the feature reports.
pub const COMMAND_USAGE_PAGE: u16 = 0x0B;
/// Top-level collection that emits event packets.
pub const EVENT_USAGE_PAGE: u16 = 0x0A;

/// The DPI report has room for 8 stages; the X11 uses 6.
pub const MAX_STAGES: u8 = 6;
/// Feature report that unlocks one read of another report.
pub const READ_REQUEST: u8 = 0xA0;

/// A value outside what the mouse accepts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Invalid(pub String);

type Result<T, E = Invalid> = std::result::Result<T, E>;

fn check_range<T: PartialOrd + fmt::Display>(name: &str, value: T, low: T, high: T) -> Result<()> {
    if low <= value && value <= high {
        Ok(())
    } else {
        Err(Invalid(format!("{name} must be between {low} and {high}, got {value}")))
    }
}

/// Bytes as space-separated hex, the way the protocol notes write packets.
pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// Store `sum(data[first..=last])` as a big-endian u16 at `at`.
fn put_sum16(data: &mut [u8], first: usize, last: usize, at: usize) {
    let sum: u32 = data[first..=last].iter().map(|&b| u32::from(b)).sum();
    data[at..at + 2].copy_from_slice(&((sum & 0xFFFF) as u16).to_be_bytes());
}

/// A configuration report held as raw bytes.
pub trait Report: Clone + fmt::Debug {
    const ID: u8;
    const LEN: usize;
    const NAME: &'static str;
    /// Whether byte 2 is the onboard profile the report belongs to.
    const PROFILE_SCOPED: bool;

    fn from_bytes(data: &[u8]) -> Result<Self>;
    fn data(&self) -> &[u8];
    fn update_checksum(&mut self);

    fn checksum_ok(&self) -> bool {
        let mut copy = self.clone();
        copy.update_checksum();
        copy.data() == self.data()
    }

    /// The bytes to send, with the checksum brought up to date.
    fn to_bytes(&self) -> Vec<u8> {
        let mut copy = self.clone();
        copy.update_checksum();
        copy.data().to_vec()
    }

    /// The profile this report belongs to, if it is profile-scoped.
    fn profile(&self) -> Option<u8> {
        Self::PROFILE_SCOPED.then(|| self.data()[2])
    }
}

/// The feature report that unlocks one GET_REPORT of `R`. `param` is the profile to read
/// (0 for the profile report itself).
pub fn read_request<R: Report>(param: u8) -> [u8; 8] {
    [READ_REQUEST, R::ID, R::LEN as u8, 0, param, 0, 0, 0]
}

macro_rules! report {
    ($(#[$meta:meta])* $name:ident, id = $id:expr, len = $len:expr, scoped = $scoped:expr) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name {
            data: [u8; $len],
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), hex(&self.data))
            }
        }

        impl Report for $name {
            const ID: u8 = $id;
            const LEN: usize = $len;
            const NAME: &'static str = stringify!($name);
            const PROFILE_SCOPED: bool = $scoped;

            fn from_bytes(data: &[u8]) -> Result<Self> {
                match data.get(..$len) {
                    Some(bytes) if bytes[0] == $id => Ok(Self {
                        data: bytes.try_into().expect("slice has the report's length"),
                    }),
                    _ => Err(Invalid(format!("not a {}: {}", stringify!($name), hex(data)))),
                }
            }

            fn data(&self) -> &[u8] {
                &self.data
            }

            fn update_checksum(&mut self) {
                Self::checksum(&mut self.data)
            }
        }
    };
}

// -- Report 0x0C: onboard profiles -------------------------------------------------------------

report!(
    /// Which onboard profile is active and how many exist. Unlike the profile-scoped reports,
    /// byte 2 here is the *active* profile.
    ProfileReport, id = 0x0C, len = 0x0A, scoped = false
);

impl ProfileReport {
    pub fn active_profile(&self) -> u8 {
        self.data[2]
    }

    pub fn set_active_profile(&mut self, profile: u32) -> Result<()> {
        check_range("profile", profile, 1, u32::from(self.profile_count()))?;
        self.data[2] = profile as u8;
        Ok(())
    }

    pub fn profile_count(&self) -> u8 {
        self.data[4]
    }

    fn checksum(data: &mut [u8; 0x0A]) {
        data[3] = !data[2];
        data[5] = !data[4];
    }
}

// -- Report 0x06: polling rate -----------------------------------------------------------------

/// Polling rate in Hz and its code.
pub const POLLING_RATES: [(u32, u8); 4] = [(125, 0x08), (250, 0x04), (500, 0x02), (1000, 0x01)];

report!(PollingRateReport, id = 0x06, len = 0x09, scoped = true);

impl PollingRateReport {
    pub fn rate_hz(&self) -> Option<u32> {
        POLLING_RATES.iter().find(|(_, code)| *code == self.data[3]).map(|(hz, _)| *hz)
    }

    pub fn set_rate_hz(&mut self, hz: u32) -> Result<()> {
        let (_, code) = POLLING_RATES
            .iter()
            .find(|(rate, _)| *rate == hz)
            .ok_or_else(|| Invalid(format!("polling rate must be one of [125, 250, 500, 1000] Hz, got {hz}")))?;
        self.data[3] = *code;
        Ok(())
    }

    fn checksum(data: &mut [u8; 0x09]) {
        data[4] = 0xFF - data[3];
    }
}

// -- Report 0x04: DPI stages, stage colors, angle snap and ripple control -----------------------

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => panic!("not a hex digit"),
    }
}

const fn hex_table<const N: usize>(hex: &[u8]) -> [u8; N] {
    let mut out = [0; N];
    let mut i = 0;
    while i < N {
        out[i] = (nibble(hex[2 * i]) << 4) | nibble(hex[2 * i + 1]);
        i += 1;
    }
    out
}

/// PAW3311 resolution codes for 50..=10000 DPI in 50-DPI steps: `BASE_CODES[i]` is the code
/// for `(i + 1) * 50` DPI.
const BASE_CODES: [u8; 200] = hex_table(
    concat!(
        "01020304050608090a0b0c0e0f1011121315161718191b1c1d1e1f20222324252627292a2b2c2d2f30313233",
        "34363738393a3b3d3e3f40414344454647484a4b4c4d4e4f51525354555758595a5b5c5e5f60616263656667",
        "68696b6c6d6e6f70727374757677797a7b7c7d7f8081828384868788898a8b8d8e8f90919394959697989a9b",
        "9c9d9e9fa1a2a3a4a5a7a8a9aaabacaeafb0b1b2b3b5b6b7b8b9bbbcbdbebfc0c2c3c4c5c6c7c9cacbcccdcf",
        "d0d1d2d3d4d6d7d8d9dadbdddedfe0e1e3e4e5e6e7e8eaeb",
    )
    .as_bytes(),
);

pub const MIN_DPI: u32 = 50;
pub const MAX_DPI: u32 = 22_000;

/// `(code, high, double)` for a DPI value.
///
/// Values above 10000 DPI reuse the base codes with one or two x2 multipliers: a per-stage
/// "high" byte (used for 10100-12000), a per-stage "double" mask bit (12100-20000), or both
/// (20200-22000). That is why larger values need coarser steps. The split between ranges mirrors
/// what the official software sends.
pub fn encode_dpi(dpi: u32) -> Result<(u8, bool, bool)> {
    check_range("DPI", dpi, MIN_DPI, MAX_DPI)?;
    let high = (10_001..=12_000).contains(&dpi) || dpi > 20_000;
    let double = dpi > 12_000;
    let factor = if high { 2 } else { 1 } * if double { 2 } else { 1 };
    if dpi % factor != 0 || (dpi / factor) % 50 != 0 {
        return Err(Invalid(format!(
            "{dpi} DPI is not supported: use steps of 50 up to 10000, 100 up to 20000, 200 above"
        )));
    }
    Ok((BASE_CODES[(dpi / factor / 50 - 1) as usize], high, double))
}

pub fn decode_dpi(code: u8, high: bool, double: bool) -> u32 {
    let nearest = BASE_CODES
        .iter()
        .enumerate()
        .min_by_key(|(_, base)| (i16::from(**base) - i16::from(code)).abs())
        .map_or(0, |(i, _)| i);
    (nearest as u32 + 1) * 50 * if high { 2 } else { 1 } * if double { 2 } else { 1 }
}

pub type Rgb = [u8; 3];

/// Parse `ff8800` or `#ff8800`.
pub fn parse_color(text: &str) -> Result<Rgb> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
    let invalid = || Invalid(format!("expected a hex color like ff8800, got '{text}'"));
    if digits.len() != 6 || !digits.is_ascii() {
        return Err(invalid());
    }
    let channel = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).map_err(|_| invalid());
    Ok([channel(0)?, channel(2)?, channel(4)?])
}

pub fn format_color(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

report!(DpiReport, id = 0x04, len = 0x38, scoped = true);

impl DpiReport {
    fn index(stage: u32) -> Result<usize> {
        check_range("DPI stage", stage, 1, u32::from(MAX_STAGES))?;
        Ok(stage as usize - 1)
    }

    // Bytes 3 and 4 keep lift-off distance and motion sync in their high nibbles; the X11
    // firmware doesn't support either, so they are left alone.

    pub fn angle_snap(&self) -> bool {
        self.data[3] & 0x0F == 1
    }

    pub fn set_angle_snap(&mut self, on: bool) {
        self.data[3] = (self.data[3] & 0xF0) | u8::from(on);
    }

    pub fn ripple_control(&self) -> bool {
        self.data[4] & 0x0F == 1
    }

    pub fn set_ripple_control(&mut self, on: bool) {
        self.data[4] = (self.data[4] & 0xF0) | u8::from(on);
    }

    /// How many stages the DPI button cycles through (byte 5 is a stage bitmask).
    pub fn stage_count(&self) -> u8 {
        (self.data[5] & ((1 << MAX_STAGES) - 1)).count_ones() as u8
    }

    pub fn set_stage_count(&mut self, count: u32) -> Result<()> {
        check_range("stage count", count, 1, u32::from(MAX_STAGES))?;
        self.data[5] = (1u8 << count) - 1;
        self.set_active_stage(u32::from(self.active_stage()).min(count))
    }

    pub fn stage_dpi(&self, stage: u32) -> Result<u32> {
        let i = Self::index(stage)?;
        let double = (self.data[6] >> i) & 1 == 1;
        Ok(decode_dpi(self.data[8 + i], self.data[16 + i] != 0, double))
    }

    pub fn set_stage_dpi(&mut self, stage: u32, dpi: u32) -> Result<()> {
        let i = Self::index(stage)?;
        let (code, high, double) = encode_dpi(dpi)?;
        self.data[8 + i] = code;
        self.data[16 + i] = u8::from(high);
        for at in [6, 7] {
            // the device keeps two copies of the double mask
            if double {
                self.data[at] |= 1 << i;
            } else {
                self.data[at] &= !(1 << i);
            }
        }
        Ok(())
    }

    pub fn stages(&self) -> Vec<u32> {
        (1..=u32::from(MAX_STAGES)).map(|stage| self.stage_dpi(stage).expect("stage in range")).collect()
    }

    pub fn active_stage(&self) -> u8 {
        self.data[24]
    }

    pub fn set_active_stage(&mut self, stage: u32) -> Result<()> {
        check_range("active DPI stage", stage, 1, u32::from(self.stage_count()))?;
        self.data[24] = stage as u8;
        Ok(())
    }

    pub fn stage_color(&self, stage: u32) -> Result<Rgb> {
        let at = 25 + 3 * Self::index(stage)?;
        Ok([self.data[at], self.data[at + 1], self.data[at + 2]])
    }

    pub fn set_stage_color(&mut self, stage: u32, rgb: Rgb) -> Result<()> {
        let at = 25 + 3 * Self::index(stage)?;
        self.data[at..at + 3].copy_from_slice(&rgb);
        Ok(())
    }

    fn checksum(data: &mut [u8; 0x38]) {
        put_sum16(data, 3, 49, 50);
    }
}

// -- Report 0x05: lighting, sleep timers and debounce --------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightMode {
    Off = 0,
    Static = 1,
    Breathing = 2,
    Neon = 3,
    ColorBreathing = 4,
    /// Solid, in the active DPI stage's color.
    StaticDpi = 5,
    /// Breathing, in the active DPI stage's color.
    BreathingDpi = 6,
}

impl LightMode {
    pub const ALL: [LightMode; 7] = [
        Self::Off,
        Self::Static,
        Self::Breathing,
        Self::Neon,
        Self::ColorBreathing,
        Self::StaticDpi,
        Self::BreathingDpi,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Static => "static",
            Self::Breathing => "breathing",
            Self::Neon => "neon",
            Self::ColorBreathing => "color-breathing",
            Self::StaticDpi => "static-dpi",
            Self::BreathingDpi => "breathing-dpi",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.slug() == slug)
    }

    fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| *mode as u8 == code)
    }
}

report!(LightingReport, id = 0x05, len = 0x0F, scoped = true);

impl LightingReport {
    /// `None` for a mode this driver doesn't know; `mode_code` has the raw value.
    pub fn mode(&self) -> Option<LightMode> {
        LightMode::from_code(self.mode_code())
    }

    pub fn mode_code(&self) -> u8 {
        self.data[3] >> 4
    }

    pub fn set_mode(&mut self, mode: LightMode) {
        self.data[3] = ((mode as u8) << 4) | (self.data[3] & 0x0F);
    }

    /// Effect speed, 1 (slowest) to 5 (fastest). The device stores `6 - speed`.
    pub fn speed(&self) -> u8 {
        6u8.saturating_sub(self.data[4] & 0x0F)
    }

    pub fn set_speed(&mut self, speed: u32) -> Result<()> {
        check_range("speed", speed, 1, 5)?;
        self.data[4] = (self.data[4] & 0xF0) | (6 - speed as u8);
        Ok(())
    }

    pub fn brightness(&self) -> u8 {
        self.data[5] & 0x0F
    }

    pub fn set_brightness(&mut self, level: u32) -> Result<()> {
        check_range("brightness", level, 1, 8)?;
        self.data[5] = (self.data[5] & 0xF0) | level as u8;
        Ok(())
    }

    pub fn color(&self) -> Rgb {
        [self.data[6], self.data[7], self.data[8]]
    }

    pub fn set_color(&mut self, rgb: Rgb) {
        self.data[6..9].copy_from_slice(&rgb);
    }

    /// Idle time before the mouse dozes (it wakes on movement).
    pub fn sleep_minutes(&self) -> f32 {
        f32::from(self.data[9]) / 2.0
    }

    pub fn set_sleep_minutes(&mut self, minutes: f32) -> Result<()> {
        check_range("sleep time", minutes, 0.5, 30.0)?;
        let halves = minutes * 2.0;
        if halves.fract() != 0.0 {
            return Err(Invalid("sleep time must be a multiple of 0.5 minutes".into()));
        }
        self.data[9] = halves as u8;
        Ok(())
    }

    /// Idle time before deep sleep. Split across the high nibble of byte 4 and byte 5.
    pub fn deep_sleep_minutes(&self) -> u8 {
        (self.data[4] & 0xF0) | (self.data[5] >> 4)
    }

    pub fn set_deep_sleep_minutes(&mut self, minutes: u32) -> Result<()> {
        check_range("deep sleep time", minutes, 1, 60)?;
        let minutes = minutes as u8;
        self.data[4] = (minutes & 0xF0) | (self.data[4] & 0x0F);
        self.data[5] = ((minutes & 0x0F) << 4) | (self.data[5] & 0x0F);
        Ok(())
    }

    pub fn debounce_ms(&self) -> u32 {
        u32::from(self.data[10]) * 2
    }

    pub fn set_debounce_ms(&mut self, ms: u32) -> Result<()> {
        check_range("debounce", ms, 4, 50)?;
        if ms % 2 != 0 {
            return Err(Invalid("debounce must be an even number of milliseconds".into()));
        }
        self.data[10] = (ms / 2) as u8;
        Ok(())
    }

    fn checksum(data: &mut [u8; 0x0F]) {
        put_sum16(data, 3, 10, 11);
    }
}

// -- Report 0x08: button bindings ---------------------------------------------------------------

/// Physical buttons, valued by their slot number in the buttons report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Button {
    Left = 1,
    Right = 2,
    Middle = 3,
    Dpi = 6,
    Forward = 7,
    Back = 8,
    WheelUp = 17,
    WheelDown = 18,
}

impl Button {
    pub const ALL: [Button; 8] =
        [Self::Left, Self::Right, Self::Middle, Self::Dpi, Self::Forward, Self::Back, Self::WheelUp, Self::WheelDown];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
            Self::Dpi => "dpi",
            Self::Forward => "forward",
            Self::Back => "back",
            Self::WheelUp => "wheel-up",
            Self::WheelDown => "wheel-down",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|button| button.slug() == slug)
    }

    /// What the button does out of the box.
    pub fn default_action(self) -> Action {
        match self {
            Self::Left => Action::LeftClick,
            Self::Right => Action::RightClick,
            Self::Middle => Action::MiddleClick,
            Self::Dpi => Action::DpiCycle,
            Self::Forward => Action::Forward,
            Self::Back => Action::Back,
            Self::WheelUp => Action::ScrollUp,
            Self::WheelDown => Action::ScrollDown,
        }
    }
}

macro_rules! actions {
    ($($variant:ident = $code:literal, $slug:literal;)*) => {
        /// First byte of a button slot.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u8)]
        pub enum Action {
            $($variant = $code,)*
        }

        impl Action {
            pub const ALL: &'static [Action] = &[$(Self::$variant,)*];

            pub fn slug(self) -> &'static str {
                match self {
                    $(Self::$variant => $slug,)*
                }
            }
        }
    };
}

actions! {
    Unset = 0x00, "unset";
    Disabled = 0x01, "disabled";
    LeftClick = 0x02, "left-click";
    RightClick = 0x03, "right-click";
    MiddleClick = 0x04, "middle-click";
    Back = 0x05, "back";
    Forward = 0x06, "forward";
    DoubleClick = 0x07, "double-click";
    Fire = 0x08, "fire";
    ScrollUp = 0x09, "scroll-up";
    ScrollDown = 0x0A, "scroll-down";
    DpiCycle = 0x0D, "dpi-cycle";
    DpiUp = 0x0E, "dpi-up";
    DpiDown = 0x0F, "dpi-down";
    EasyAim = 0x10, "easy-aim";
    Key = 0x11, "key";
    Macro = 0x12, "macro";
    MediaPlayer = 0x15, "media-player";
    PreviousTrack = 0x16, "previous-track";
    NextTrack = 0x17, "next-track";
    PlayPause = 0x18, "play-pause";
    Stop = 0x19, "stop";
    Mute = 0x1A, "mute";
    VolumeUp = 0x1B, "volume-up";
    VolumeDown = 0x1C, "volume-down";
    Calculator = 0x1D, "calculator";
    Email = 0x1E, "email";
    BrowserForward = 0x20, "browser-forward";
    BrowserBack = 0x21, "browser-back";
    BrowserStop = 0x22, "browser-stop";
    MyComputer = 0x23, "my-computer";
    BrowserRefresh = 0x24, "browser-refresh";
    BrowserHome = 0x25, "browser-home";
    BrowserSearch = 0x26, "browser-search";
    ProfileCycle = 0x34, "profile-cycle";
    ProfileUp = 0x35, "profile-up";
    ProfileDown = 0x36, "profile-down";
    PollingRateCycle = 0x40, "polling-rate-cycle";
}

impl Action {
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.iter().copied().find(|action| *action as u8 == code)
    }

    /// Actions that can be assigned by name. Left out: `Key` (use `key:<combo>`), `Fire`,
    /// `EasyAim` and `Macro` (they need parameters or macro data), and the profile actions
    /// (they can strand you on a profile whose buttons lack the action).
    pub fn assignable() -> impl Iterator<Item = Action> {
        use Action::*;
        Self::ALL.iter().copied().filter(|action| {
            !matches!(action, Unset | Key | Fire | EasyAim | Macro | ProfileCycle | ProfileUp | ProfileDown)
        })
    }
}

/// Modifier names and their bits in a key binding.
pub const MODIFIERS: [(&str, u8); 4] = [("ctrl", 0x01), ("shift", 0x02), ("alt", 0x04), ("win", 0x08)];

/// Key names and their USB HID keyboard usage IDs.
pub fn keys() -> &'static [(String, u8)] {
    static KEYS: OnceLock<Vec<(String, u8)>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let mut keys = Vec::new();
        for i in 0..26u8 {
            keys.push((char::from(b'a' + i).to_string(), 0x04 + i));
        }
        for i in 0..10u8 {
            keys.push((((i + 1) % 10).to_string(), 0x1E + i));
        }
        for i in 1..=12u8 {
            keys.push((format!("f{i}"), 0x39 + i));
        }
        let named = "enter esc backspace tab space minus equal leftbracket rightbracket backslash \
                     nonushash semicolon quote grave comma period slash capslock";
        keys.extend(named.split_whitespace().zip(0x28..).map(|(name, usage)| (name.to_string(), usage)));
        let named = "printscreen scrolllock pause insert home pageup delete end pagedown right left down up";
        keys.extend(named.split_whitespace().zip(0x46..).map(|(name, usage)| (name.to_string(), usage)));
        keys
    })
}

fn key_usage(name: &str) -> Option<u8> {
    keys().iter().find(|(key, _)| key == name).map(|(_, usage)| *usage)
}

fn key_name(usage: u8) -> Option<&'static str> {
    keys().iter().find(|(_, key)| *key == usage).map(|(name, _)| name.as_str())
}

/// What a button slot does: an action plus, for `Key`, a modifier mask and a key usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub action: u8,
    pub modifiers: u8,
    pub key: u8,
}

impl From<Action> for Binding {
    fn from(action: Action) -> Self {
        Self { action: action as u8, modifiers: 0, key: 0 }
    }
}

impl Binding {
    /// Parse an action name (`back`, `dpi-cycle`) or a key combo (`key:ctrl+shift+t`).
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim().to_lowercase();
        if let Some(combo) = text.strip_prefix("key:") {
            let mut parts: Vec<&str> = combo.split('+').collect();
            let key = key_usage(parts.pop().unwrap_or_default());
            let mut modifiers = 0;
            let mut known = key.is_some();
            for part in parts {
                match MODIFIERS.iter().find(|(name, _)| *name == part) {
                    Some((_, bit)) => modifiers |= bit,
                    None => known = false,
                }
            }
            return match (known, key) {
                (true, Some(key)) => Ok(Self { action: Action::Key as u8, modifiers, key }),
                _ => Err(Invalid(format!("unknown key combo '{text}'"))),
            };
        }
        Action::assignable()
            .find(|action| action.slug() == text)
            .map(Self::from)
            .ok_or_else(|| Invalid(format!("unknown action '{text}' (see --list-actions)")))
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.action == Action::Key as u8 {
            let mut parts: Vec<String> = MODIFIERS
                .iter()
                .filter(|(_, bit)| self.modifiers & bit != 0)
                .map(|(name, _)| (*name).to_string())
                .collect();
            parts.push(key_name(self.key).map_or_else(|| format!("0x{:02x}", self.key), str::to_string));
            return write!(f, "key:{}", parts.join("+"));
        }
        match Action::from_code(self.action) {
            Some(action) => f.write_str(action.slug()),
            None => write!(f, "0x{:02x}", self.action),
        }
    }
}

report!(
    /// 18 three-byte slots: action, modifiers, key usage.
    ButtonsReport, id = 0x08, len = 0x3B, scoped = true
);

impl ButtonsReport {
    pub fn binding(&self, button: Button) -> Binding {
        let at = 3 * button as usize;
        Binding { action: self.data[at], modifiers: self.data[at + 1], key: self.data[at + 2] }
    }

    pub fn set_binding(&mut self, button: Button, binding: Binding) {
        let at = 3 * button as usize;
        self.data[at..at + 3].copy_from_slice(&[binding.action, binding.modifiers, binding.key]);
    }

    /// Whether some button still clicks; a mapping without one leaves the mouse hard to use.
    pub fn has_left_click(&self) -> bool {
        Button::ALL.iter().any(|button| self.binding(*button).action == Action::LeftClick as u8)
    }

    fn checksum(data: &mut [u8; 0x3B]) {
        put_sum16(data, 3, 56, 57);
    }
}

// -- Event packets (input report 0x03): 03 <model id> <code> <param1> <param2> ------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryState {
    Discharging = 1,
    Full = 2,
    Charging = 3,
}

impl BatteryState {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Discharging => "discharging",
            Self::Full => "full",
            Self::Charging => "charging",
        }
    }
}

/// A battery report, sent every 2 s or so while the mouse is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// Percent.
    pub level: u8,
    /// The raw state byte; see [`Battery::state`].
    pub state_code: u8,
}

impl Battery {
    pub fn state(&self) -> Option<BatteryState> {
        match self.state_code {
            1 => Some(BatteryState::Discharging),
            2 => Some(BatteryState::Full),
            3 => Some(BatteryState::Charging),
            _ => None,
        }
    }
}

impl fmt::Display for Battery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.state() {
            Some(BatteryState::Full) => write!(f, "{}% (fully charged)", self.level),
            Some(state) => write!(f, "{}% ({})", self.level, state.slug()),
            None => write!(f, "{}% (state 0x{:02x})", self.level, self.state_code),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Battery(Battery),
    /// Sent after every feature-report write.
    Ack {
        report_id: u8,
        ok: bool,
    },
    /// The DPI button switched to this stage.
    DpiStage(u8),
    /// The active profile changed on the mouse (1-based).
    Profile(u8),
}

pub fn parse_event(packet: &[u8]) -> Option<Event> {
    let [0x03, _model, code, param1, param2, ..] = *packet else {
        return None;
    };
    match code {
        0x40 | 0x41 => Some(Event::Battery(Battery { level: param2, state_code: param1 })),
        0x50 => Some(Event::Ack { report_id: param2, ok: param1 == 0 }),
        0x10 => Some(Event::DpiStage(param1)),
        0x80 => Some(Event::Profile(param1 + 1)), // 0-based on the wire
        _ => None,
    }
}
