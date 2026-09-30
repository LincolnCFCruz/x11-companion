//! `shark-x11`: configure the Attack Shark X11 from the command line.
//!
//! Every setting command prints the current value when run without arguments. Changes are
//! read-modify-write on the target profile (the active one unless --profile is given), so
//! settings a command doesn't mention are left as they are.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::Duration;

use attack_shark_x11::protocol::{
    Action, Button, ButtonsReport, DpiReport, Event, LightMode, LightingReport, MAX_STAGES, PollingRateReport,
    ProfileReport, format_color, keys,
};
use attack_shark_x11::settings::{
    ButtonsPatch, DpiPatch, LightingPatch, apply_buttons, apply_dpi, apply_lighting, apply_polling_rate, apply_profile,
};
use attack_shark_x11::{Device, Error, Invalid};
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "shark-x11",
    version,
    about = "Configure the Attack Shark X11 mouse and read its battery level.",
    long_about = "Configure the Attack Shark X11 mouse and read its battery level. \
                  Setting commands without arguments show the current value."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Args)]
struct Target {
    /// Onboard profile to act on (default: the active one)
    #[arg(long, value_name = "N")]
    profile: Option<u8>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show battery level and all settings
    Status {
        #[command(flatten)]
        target: Target,
    },
    /// Show the battery level
    Battery {
        /// Keep printing whenever the level or charging state changes
        #[arg(long)]
        watch: bool,
        /// Seconds to wait for a report
        #[arg(long, default_value_t = 5.0)]
        timeout: f64,
    },
    /// Set DPI stages, the active stage and stage colors
    Dpi {
        /// DPI for stages 1, 2, ... (50-22000; steps of 50 to 10000, 100 to 20000, 200 above)
        values: Vec<u32>,
        /// Switch to this stage
        #[arg(long, value_name = "STAGE")]
        active: Option<u32>,
        /// Number of stages the DPI button cycles through (1-6)
        #[arg(long, value_name = "N")]
        count: Option<u32>,
        /// LED colors for stages 1, 2, ... (used by the static-dpi and breathing-dpi lighting modes)
        #[arg(long, num_args = 1.., value_name = "HEX")]
        colors: Vec<String>,
        #[command(flatten)]
        target: Target,
    },
    /// Set the polling rate
    PollingRate {
        /// 125, 250, 500 or 1000
        #[arg(value_parser = ["125", "250", "500", "1000"])]
        hz: Option<String>,
        #[command(flatten)]
        target: Target,
    },
    /// Set the LED effect
    Lighting {
        /// off, static, breathing, neon, color-breathing, static-dpi, breathing-dpi
        #[arg(value_parser = LightMode::ALL.map(LightMode::slug))]
        mode: Option<String>,
        /// Color for static and breathing, e.g. ff8800
        #[arg(long, value_name = "HEX")]
        color: Option<String>,
        /// Effect speed, 5 is fastest
        #[arg(long, value_name = "1-5")]
        speed: Option<u32>,
        /// LED brightness
        #[arg(long, value_name = "1-8")]
        brightness: Option<u32>,
        #[command(flatten)]
        target: Target,
    },
    /// Set angle snapping and ripple control
    Sensor {
        #[arg(long, value_name = "on|off", value_parser = on_off)]
        angle_snap: Option<bool>,
        #[arg(long, value_name = "on|off", value_parser = on_off)]
        ripple: Option<bool>,
        #[command(flatten)]
        target: Target,
    },
    /// Set the sleep timers
    Power {
        /// Idle minutes before sleep (0.5-30, steps of 0.5)
        #[arg(long, value_name = "MIN")]
        sleep: Option<f32>,
        /// Idle minutes before deep sleep (1-60)
        #[arg(long, value_name = "MIN")]
        deep_sleep: Option<u32>,
        #[command(flatten)]
        target: Target,
    },
    /// Set the button debounce time
    Debounce {
        /// 4-50, even numbers only
        ms: Option<u32>,
        #[command(flatten)]
        target: Target,
    },
    /// Remap buttons
    Buttons {
        /// e.g. forward=key:ctrl+c back=play-pause dpi=dpi-up
        #[arg(value_name = "BUTTON=ACTION")]
        assignments: Vec<String>,
        /// Restore the default mapping first
        #[arg(long)]
        reset: bool,
        /// List the actions and keys you can assign
        #[arg(long)]
        list_actions: bool,
        /// Allow a mapping with no left-click button
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        target: Target,
    },
    /// Switch the active onboard profile
    Profile {
        #[arg(value_name = "N")]
        number: Option<u32>,
        /// Switch even if the profile looks uninitialized
        #[arg(long)]
        force: bool,
    },
}

fn on_off(text: &str) -> Result<bool, String> {
    match text.to_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Ok(true),
        "off" | "false" | "0" | "no" => Ok(false),
        _ => Err("expected on or off".into()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::Buttons { list_actions: true, .. } = cli.command {
        print_assignable_actions();
        return ExitCode::SUCCESS;
    }
    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<(), Error> {
    let mut mouse = Device::open()?;
    let profile = match &command {
        Command::Status { target }
        | Command::Dpi { target, .. }
        | Command::PollingRate { target, .. }
        | Command::Lighting { target, .. }
        | Command::Sensor { target, .. }
        | Command::Power { target, .. }
        | Command::Debounce { target, .. }
        | Command::Buttons { target, .. } => target.profile,
        Command::Battery { .. } | Command::Profile { .. } => None,
    };
    if let Some(profile) = profile {
        let count = mouse.read::<ProfileReport>(None)?.profile_count();
        if !(1..=count).contains(&profile) {
            return Err(Invalid(format!("--profile must be between 1 and {count}")).into());
        }
    }

    match command {
        Command::Status { .. } => status(&mut mouse, profile)?,
        Command::Battery { watch, timeout } => battery(&mut mouse, watch, timeout)?,
        Command::Dpi { values, active, count, colors, .. } => {
            if values.len() > usize::from(MAX_STAGES) {
                return Err(Invalid(format!("the X11 has {MAX_STAGES} DPI stages, got {} values", values.len())).into());
            }
            let show_colors = !colors.is_empty();
            let patch = DpiPatch { stages: Some(values), count, active, colors: Some(colors), ..Default::default() };
            apply_dpi(&mut mouse, profile, &patch)?;
            let report: DpiReport = mouse.read(profile)?;
            println!("{}", describe_dpi(&report));
            if show_colors {
                let colors: Vec<String> = (1..=u32::from(report.stage_count()))
                    .map(|stage| format_color(report.stage_color(stage).expect("stage in range")))
                    .collect();
                println!("colors: {}", colors.join("  "));
            }
        }
        Command::PollingRate { hz, .. } => {
            let hz = hz.map(|hz| hz.parse().expect("one of the allowed values"));
            let rate = apply_polling_rate(&mut mouse, profile, hz)?;
            println!("{} Hz", rate.map_or_else(|| "unknown".into(), |hz| hz.to_string()));
        }
        Command::Lighting { mode, color, speed, brightness, .. } => {
            let patch = LightingPatch { mode, color, speed, brightness, ..Default::default() };
            apply_lighting(&mut mouse, profile, &patch)?;
            println!("{}", describe_lighting(&mouse.read(profile)?));
        }
        Command::Sensor { angle_snap, ripple, .. } => {
            let view = apply_dpi(&mut mouse, profile, &DpiPatch { angle_snap, ripple, ..Default::default() })?;
            println!("angle snap {}, ripple control {}", on_off_text(view.angle_snap), on_off_text(view.ripple));
        }
        Command::Power { sleep, deep_sleep, .. } => {
            let view = apply_lighting(&mut mouse, profile, &LightingPatch { sleep, deep_sleep, ..Default::default() })?;
            println!("sleep after {} min, deep sleep after {} min", view.sleep, view.deep_sleep);
        }
        Command::Debounce { ms, .. } => {
            let view = apply_lighting(&mut mouse, profile, &LightingPatch { debounce: ms, ..Default::default() })?;
            println!("{} ms", view.debounce);
        }
        Command::Buttons { assignments, reset, force, .. } => {
            let mut bindings = BTreeMap::new();
            for assignment in assignments {
                let names: Vec<&str> = Button::ALL.map(Button::slug).to_vec();
                match assignment.split_once('=') {
                    Some((name, action)) if names.contains(&name.to_lowercase().as_str()) => {
                        bindings.insert(name.to_lowercase(), action.to_string());
                    }
                    _ => {
                        return Err(Invalid(format!(
                            "expected BUTTON=ACTION with BUTTON one of {}, got '{assignment}'",
                            names.join(", ")
                        ))
                        .into());
                    }
                }
            }
            let buttons = apply_buttons(&mut mouse, profile, &ButtonsPatch { reset, bindings }, force)
                .map_err(|error| with_hint(error, "Pass --force to do it anyway."))?;
            for button in Button::ALL {
                println!("{:>10} = {}", button.slug(), buttons[button.slug()]);
            }
        }
        Command::Profile { number, force } => {
            let view = apply_profile(&mut mouse, number, force)
                .map_err(|error| with_hint(error, "Pass --force to switch anyway."))?;
            println!("profile {} of {}", view.active, view.count);
        }
    }
    Ok(())
}

fn with_hint(error: Error, hint: &str) -> Error {
    match error {
        Error::Guard(message) => Error::Guard(format!("{message} {hint}")),
        other => other,
    }
}

fn on_off_text(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn status(mouse: &mut Device, profile: Option<u8>) -> Result<(), Error> {
    let profiles: ProfileReport = mouse.read(None)?;
    let profile = profile.unwrap_or(profiles.active_profile());
    let polling: PollingRateReport = mouse.read(Some(profile))?;
    let dpi: DpiReport = mouse.read(Some(profile))?;
    let lighting: LightingReport = mouse.read(Some(profile))?;
    let buttons: ButtonsReport = mouse.read(Some(profile))?;
    let battery = match mouse.battery() {
        Some(battery) => Ok(battery),
        None => mouse.read_battery(Duration::from_secs(3)),
    }
    .map_or_else(|_| "unknown (no report; the mouse may be asleep)".into(), |battery| battery.to_string());

    let stages_note = if dpi.stage_count() < MAX_STAGES {
        format!("  ({} of {MAX_STAGES} stages enabled)", dpi.stage_count())
    } else {
        String::new()
    };
    let rows = [
        ("Connection", format!("{} (1d57:{:04x})", mouse.connection(), mouse.product_id())),
        ("Battery", battery),
        ("Profile", format!("{profile} (active {}, {} total)", profiles.active_profile(), profiles.profile_count())),
        ("Polling rate", polling.rate_hz().map_or_else(|| "unknown".into(), |hz| format!("{hz} Hz"))),
        ("DPI", describe_dpi(&dpi) + &stages_note),
        ("Angle snap", on_off_text(dpi.angle_snap()).into()),
        ("Ripple ctrl", on_off_text(dpi.ripple_control()).into()),
        ("Lighting", describe_lighting(&lighting)),
        ("Debounce", format!("{} ms", lighting.debounce_ms())),
        ("Sleep", format!("{} min, deep sleep {} min", lighting.sleep_minutes(), lighting.deep_sleep_minutes())),
    ];
    for (label, value) in rows {
        println!("{label:<13} {value}");
    }
    println!("Buttons");
    for button in Button::ALL {
        println!("  {:>10} = {}", button.slug(), buttons.binding(button));
    }
    Ok(())
}

fn battery(mouse: &mut Device, watch: bool, timeout: f64) -> Result<(), Error> {
    if !watch {
        println!("{}", mouse.read_battery(Duration::from_secs_f64(timeout))?);
        return Ok(());
    }
    let mut last = None;
    loop {
        if let Some(Event::Battery(battery)) = mouse.wait_for(None, |event| matches!(event, Event::Battery(_)))? {
            if last != Some(battery) {
                println!("{}  {battery}", chrono::Local::now().format("%H:%M:%S"));
                last = Some(battery);
            }
        }
    }
}

fn describe_dpi(report: &DpiReport) -> String {
    let active = u32::from(report.active_stage());
    (1..)
        .zip(report.stages())
        .take(usize::from(report.stage_count()))
        .map(|(stage, dpi)| if stage == active { format!("[{dpi}]") } else { dpi.to_string() })
        .collect::<Vec<_>>()
        .join("  ")
}

fn describe_lighting(report: &LightingReport) -> String {
    let Some(mode) = report.mode() else {
        return format!("0x{:x} (brightness {}/8)", report.mode_code(), report.brightness());
    };
    if mode == LightMode::Off {
        return "off".into();
    }
    let mut details = Vec::new();
    if matches!(mode, LightMode::Static | LightMode::Breathing) {
        details.push(format!("color {}", format_color(report.color())));
    }
    details.push(format!("brightness {}/8", report.brightness()));
    if !matches!(mode, LightMode::Static | LightMode::StaticDpi) {
        details.push(format!("speed {}/5", report.speed()));
    }
    format!("{} ({})", mode.slug(), details.join(", "))
}

fn print_assignable_actions() {
    let actions: Vec<&str> = Action::assignable().map(Action::slug).collect();
    println!("actions: {}", actions.join(", "));
    println!("keys:    key:[ctrl+][shift+][alt+][win+]<key>, where <key> is one of");
    let names: Vec<&str> = keys().iter().map(|(name, _)| name.as_str()).collect();
    println!("         {}", names.join(", "));
}
