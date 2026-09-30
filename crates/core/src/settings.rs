//! Settings as the app and the CLI see them.
//!
//! Every change is a read-modify-write on one profile (the active one unless given), so settings
//! a change doesn't mention are left as they are, and nothing is written when nothing changed.
//! Changes that would leave the mouse hard to use are refused unless forced.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::protocol::{
    Action, Binding, Button, ButtonsReport, DpiReport, LightMode, LightingReport, MAX_DPI, MAX_STAGES, MIN_DPI,
    POLLING_RATES, PollingRateReport, ProfileReport, Report, format_color, keys, parse_color,
};
use crate::transport::Transport;
use crate::{Device, Error, Invalid};

/// What the settings layer needs from a mouse; tests implement it with a fake.
pub trait Mouse {
    fn read<R: Report>(&mut self, profile: Option<u8>) -> Result<R, Error>;
    fn write<R: Report>(&mut self, report: &R) -> Result<(), Error>;
}

impl<T: Transport> Mouse for Device<T> {
    fn read<R: Report>(&mut self, profile: Option<u8>) -> Result<R, Error> {
        Device::<T>::read(self, profile)
    }

    fn write<R: Report>(&mut self, report: &R) -> Result<(), Error> {
        Device::<T>::write(self, report)
    }
}

// -- views ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct State {
    pub profile: ProfileView,
    pub polling_rate: Option<u32>,
    pub dpi: DpiView,
    pub lighting: LightingView,
    pub buttons: Buttons,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProfileView {
    pub active: u8,
    pub count: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DpiView {
    pub stages: Vec<u32>,
    pub active: u8,
    pub count: u8,
    pub colors: Vec<String>,
    pub angle_snap: bool,
    pub ripple: bool,
}

impl From<&DpiReport> for DpiView {
    fn from(report: &DpiReport) -> Self {
        Self {
            stages: report.stages(),
            active: report.active_stage(),
            count: report.stage_count(),
            colors: (1..=u32::from(MAX_STAGES))
                .map(|stage| format_color(report.stage_color(stage).expect("stage in range")))
                .collect(),
            angle_snap: report.angle_snap(),
            ripple: report.ripple_control(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LightingView {
    /// `None` for a mode this driver doesn't know.
    pub mode: Option<&'static str>,
    pub color: String,
    pub speed: u8,
    pub brightness: u8,
    pub sleep: f32,
    pub deep_sleep: u8,
    pub debounce: u32,
}

impl From<&LightingReport> for LightingView {
    fn from(report: &LightingReport) -> Self {
        Self {
            mode: report.mode().map(LightMode::slug),
            color: format_color(report.color()),
            speed: report.speed(),
            brightness: report.brightness(),
            sleep: report.sleep_minutes(),
            deep_sleep: report.deep_sleep_minutes(),
            debounce: report.debounce_ms(),
        }
    }
}

/// Button name to binding, e.g. `forward` to `key:ctrl+c`.
pub type Buttons = BTreeMap<&'static str, String>;

pub fn buttons_view(report: &ButtonsReport) -> Buttons {
    Button::ALL.into_iter().map(|button| (button.slug(), report.binding(button).to_string())).collect()
}

/// Everything the settings window shows, for the active profile.
pub fn read_state<M: Mouse>(mouse: &mut M) -> Result<State, Error> {
    let profiles: ProfileReport = mouse.read(None)?;
    let profile = Some(profiles.active_profile());
    Ok(State {
        profile: ProfileView { active: profiles.active_profile(), count: profiles.profile_count() },
        polling_rate: mouse.read::<PollingRateReport>(profile)?.rate_hz(),
        dpi: DpiView::from(&mouse.read::<DpiReport>(profile)?),
        lighting: LightingView::from(&mouse.read::<LightingReport>(profile)?),
        buttons: buttons_view(&mouse.read::<ButtonsReport>(profile)?),
    })
}

/// The option lists the settings window offers.
#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub actions: Vec<&'static str>,
    pub keys: Vec<&'static str>,
    pub light_modes: Vec<&'static str>,
    pub polling_rates: Vec<u32>,
    pub buttons: Vec<&'static str>,
    pub max_stages: u8,
    pub dpi: DpiRange,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DpiRange {
    pub min: u32,
    pub max: u32,
}

pub fn meta() -> Meta {
    let mut polling_rates: Vec<u32> = POLLING_RATES.iter().map(|(hz, _)| *hz).collect();
    polling_rates.sort_unstable();
    Meta {
        actions: Action::assignable().map(Action::slug).collect(),
        keys: keys().iter().map(|(name, _)| name.as_str()).collect(),
        light_modes: LightMode::ALL.into_iter().map(LightMode::slug).collect(),
        polling_rates,
        buttons: Button::ALL.into_iter().map(Button::slug).collect(),
        max_stages: MAX_STAGES,
        dpi: DpiRange { min: MIN_DPI, max: MAX_DPI },
    }
}

// -- changes ---------------------------------------------------------------------------------------

fn write_if_changed<M: Mouse, R: Report + PartialEq>(mouse: &mut M, before: &R, after: &R) -> Result<(), Error> {
    if before == after { Ok(()) } else { mouse.write(after) }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DpiPatch {
    /// How many stages the DPI button cycles through; applied before `active`.
    pub count: Option<u32>,
    /// DPI for stages 1, 2, ...
    pub stages: Option<Vec<u32>>,
    pub active: Option<u32>,
    /// LED colors for stages 1, 2, ...
    pub colors: Option<Vec<String>>,
    pub angle_snap: Option<bool>,
    pub ripple: Option<bool>,
}

pub fn apply_dpi<M: Mouse>(mouse: &mut M, profile: Option<u8>, patch: &DpiPatch) -> Result<DpiView, Error> {
    let before: DpiReport = mouse.read(profile)?;
    let mut report = before.clone();
    if let Some(count) = patch.count {
        report.set_stage_count(count)?;
    }
    for (stage, dpi) in (1..).zip(patch.stages.iter().flatten()) {
        report.set_stage_dpi(stage, *dpi)?;
    }
    if let Some(active) = patch.active {
        report.set_active_stage(active)?;
    }
    for (stage, color) in (1..).zip(patch.colors.iter().flatten()) {
        report.set_stage_color(stage, parse_color(color)?)?;
    }
    if let Some(on) = patch.angle_snap {
        report.set_angle_snap(on);
    }
    if let Some(on) = patch.ripple {
        report.set_ripple_control(on);
    }
    write_if_changed(mouse, &before, &report)?;
    Ok(DpiView::from(&report))
}

pub fn apply_polling_rate<M: Mouse>(mouse: &mut M, profile: Option<u8>, hz: Option<u32>) -> Result<Option<u32>, Error> {
    let before: PollingRateReport = mouse.read(profile)?;
    let mut report = before.clone();
    if let Some(hz) = hz {
        report.set_rate_hz(hz)?;
    }
    write_if_changed(mouse, &before, &report)?;
    Ok(report.rate_hz())
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LightingPatch {
    pub mode: Option<String>,
    pub color: Option<String>,
    pub speed: Option<u32>,
    pub brightness: Option<u32>,
    /// Minutes before sleep, in steps of 0.5.
    pub sleep: Option<f32>,
    /// Minutes before deep sleep.
    pub deep_sleep: Option<u32>,
    /// Debounce in milliseconds.
    pub debounce: Option<u32>,
}

pub fn apply_lighting<M: Mouse>(
    mouse: &mut M,
    profile: Option<u8>,
    patch: &LightingPatch,
) -> Result<LightingView, Error> {
    let before: LightingReport = mouse.read(profile)?;
    let mut report = before.clone();
    if let Some(mode) = &patch.mode {
        let mode = LightMode::from_slug(mode).ok_or_else(|| Invalid(format!("unknown lighting mode '{mode}'")))?;
        report.set_mode(mode);
    }
    if let Some(color) = &patch.color {
        report.set_color(parse_color(color)?);
    }
    if let Some(speed) = patch.speed {
        report.set_speed(speed)?;
    }
    if let Some(level) = patch.brightness {
        report.set_brightness(level)?;
    }
    if let Some(minutes) = patch.sleep {
        report.set_sleep_minutes(minutes)?;
    }
    if let Some(minutes) = patch.deep_sleep {
        report.set_deep_sleep_minutes(minutes)?;
    }
    if let Some(ms) = patch.debounce {
        report.set_debounce_ms(ms)?;
    }
    write_if_changed(mouse, &before, &report)?;
    Ok(LightingView::from(&report))
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ButtonsPatch {
    /// Restore the default binding of every button first.
    pub reset: bool,
    /// Button name to action, e.g. `back` to `play-pause` or `forward` to `key:ctrl+c`.
    pub bindings: BTreeMap<String, String>,
}

pub const NO_LEFT_CLICK: &str = "At least one button has to stay on left click.";

/// Apply button bindings. Unless `force`, refuses a mapping that leaves no button on left click.
pub fn apply_buttons<M: Mouse>(
    mouse: &mut M,
    profile: Option<u8>,
    patch: &ButtonsPatch,
    force: bool,
) -> Result<Buttons, Error> {
    let before: ButtonsReport = mouse.read(profile)?;
    let mut report = before.clone();
    if patch.reset {
        for button in Button::ALL {
            report.set_binding(button, button.default_action().into());
        }
    }
    for (name, action) in &patch.bindings {
        let button = Button::from_slug(name).ok_or_else(|| Invalid(format!("unknown button '{name}'")))?;
        report.set_binding(button, Binding::parse(action)?);
    }
    if !force && !report.has_left_click() {
        return Err(Error::Guard(NO_LEFT_CLICK.into()));
    }
    write_if_changed(mouse, &before, &report)?;
    Ok(buttons_view(&report))
}

/// Switch the active profile. Unless `force`, refuses a profile without a left-click button,
/// which is what a never-configured profile looks like.
pub fn apply_profile<M: Mouse>(mouse: &mut M, profile: Option<u32>, force: bool) -> Result<ProfileView, Error> {
    let before: ProfileReport = mouse.read(None)?;
    let mut report = before.clone();
    if let Some(profile) = profile {
        report.set_active_profile(profile)?;
        let buttons: ButtonsReport = mouse.read(Some(report.active_profile()))?;
        if !force && !buttons.has_left_click() {
            return Err(Error::Guard(format!("Profile {profile} has no left-click button, so it looks unset.")));
        }
    }
    write_if_changed(mouse, &before, &report)?;
    Ok(ProfileView { active: report.active_profile(), count: report.profile_count() })
}
