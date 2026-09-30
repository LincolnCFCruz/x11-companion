//! Starting at login, in the tray.
//!
//! - Windows: a per-user Run entry. Written directly rather than through a plugin, so the command
//!   is quoted (the path may have spaces) and a "disabled" set in Task Manager is taken into account.
//! - macOS: a LaunchAgent.
//! - Linux: an XDG autostart entry.

#[cfg(not(windows))]
use std::{fs, io, path::Path};

/// When the app starts: "Start with Windows", or "Start at login" elsewhere.
pub const WHEN: &str = if cfg!(windows) { "with Windows" } else { "at login" };

#[cfg(windows)]
mod platform {
    use std::io;

    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    /// Where Task Manager and Settings record a startup entry as disabled (odd first byte).
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    const NAME: &str = "Attack Shark X11";

    fn command() -> io::Result<String> {
        Ok(format!("\"{}\" {}", std::env::current_exe()?.display(), crate::BACKGROUND_FLAG))
    }

    pub fn is_enabled() -> Option<bool> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let present = user.open_subkey(RUN).and_then(|run| run.get_raw_value(NAME)).is_ok();
        let disabled = user
            .open_subkey(APPROVED)
            .and_then(|approved| approved.get_raw_value(NAME))
            .is_ok_and(|flags| flags.bytes.first().is_some_and(|flag| flag % 2 == 1));
        Some(present && !disabled)
    }

    pub fn set_enabled(on: bool) -> io::Result<()> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let (run, _) = user.create_subkey(RUN)?;
        if on {
            run.set_value(NAME, &command()?)?;
        } else if let Err(error) = run.delete_value(NAME) {
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error);
            }
        }
        // Forget an earlier "disabled" from Task Manager, so the switch means what it says.
        if let Ok(approved) = user.open_subkey_with_flags(APPROVED, KEY_SET_VALUE) {
            let _ = approved.delete_value(NAME);
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::PathBuf;
    use std::{env, fs, io};

    const LABEL: &str = "com.lincolncruz.attacksharkx11";

    fn agent() -> Option<PathBuf> {
        Some(PathBuf::from(env::var_os("HOME")?).join(format!("Library/LaunchAgents/{LABEL}.plist")))
    }

    pub fn is_enabled() -> Option<bool> {
        Some(agent()?.exists())
    }

    pub fn set_enabled(on: bool) -> io::Result<()> {
        let agent = agent().ok_or_else(|| io::Error::other("HOME isn't set"))?;
        if !on {
            return super::remove(&agent);
        }
        let exe = env::current_exe()?;
        // macOS runs a downloaded app from a temporary location until it's moved, and that path is
        // gone once the app quits.
        if exe.to_string_lossy().contains("/AppTranslocation/") {
            return Err(io::Error::other("move Attack Shark X11 to the Applications folder first"));
        }
        fs::create_dir_all(agent.parent().expect("the agent has a folder"))?;
        fs::write(&agent, super::launch_agent(LABEL, &exe.to_string_lossy()))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use std::path::PathBuf;
    use std::{env, fs, io};

    fn entry() -> Option<PathBuf> {
        let config = match env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => PathBuf::from(env::var_os("HOME")?).join(".config"),
        };
        Some(config.join("autostart/attack-shark-x11.desktop"))
    }

    pub fn is_enabled() -> Option<bool> {
        let Ok(text) = fs::read_to_string(entry()?) else { return Some(false) };
        // Desktops switch an entry off with one of these rather than deleting it.
        Some(!text.lines().any(|line| matches!(line.trim(), "Hidden=true" | "X-GNOME-Autostart-enabled=false")))
    }

    pub fn set_enabled(on: bool) -> io::Result<()> {
        let entry = entry().ok_or_else(|| io::Error::other("HOME isn't set"))?;
        if !on {
            return super::remove(&entry);
        }
        // In an AppImage the binary runs from a temporary mount: start the AppImage itself.
        let program = match env::var_os("APPIMAGE") {
            Some(appimage) => PathBuf::from(appimage),
            None => env::current_exe()?,
        };
        fs::create_dir_all(entry.parent().expect("the entry has a folder"))?;
        fs::write(&entry, super::desktop_entry(&program.to_string_lossy()))
    }
}

pub use platform::{is_enabled, set_enabled};

/// Delete `file`, which may already be gone.
#[cfg(not(windows))]
fn remove(file: &Path) -> io::Result<()> {
    match fs::remove_file(file) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// A LaunchAgent that starts `program` in the tray at login.
#[cfg(any(target_os = "macos", test))]
fn launch_agent(label: &str, program: &str) -> String {
    let program = program.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{program}</string>
    <string>{flag}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#,
        flag = crate::BACKGROUND_FLAG
    )
}

/// An XDG autostart entry that starts `program` in the tray.
#[cfg(any(not(any(windows, target_os = "macos")), test))]
fn desktop_entry(program: &str) -> String {
    // Exec takes the path quoted, with " ` $ and \ escaped by a backslash; the file format then
    // doubles every backslash, and % is written %%.
    let mut exec = String::new();
    for c in program.chars() {
        match c {
            '"' | '`' | '$' => {
                exec.push_str(r"\\");
                exec.push(c);
            }
            '\\' => exec.push_str(r"\\\\"),
            '%' => exec.push_str("%%"),
            _ => exec.push(c),
        }
    }
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec=\"{exec}\" {}\n",
        crate::tray::TITLE,
        crate::BACKGROUND_FLAG
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_quotes_the_program() {
        let entry = desktop_entry("/home/me/Apps/Attack Shark X11.AppImage");
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=\"/home/me/Apps/Attack Shark X11.AppImage\" --background\n"));
    }

    #[test]
    fn desktop_entry_escapes_reserved_characters() {
        let entry = desktop_entry(r#"/tmp/100% "odd" $dir\x"#);
        assert!(entry.contains(r#"Exec="/tmp/100%% \\"odd\\" \\$dir\\\\x" --background"#));
    }

    #[test]
    fn launch_agent_escapes_the_program() {
        let agent = launch_agent("label", "/Applications/A & B.app/Contents/MacOS/A <1>");
        assert!(agent.contains("<string>/Applications/A &amp; B.app/Contents/MacOS/A &lt;1&gt;</string>"));
        assert!(agent.contains("<string>--background</string>"));
    }
}
