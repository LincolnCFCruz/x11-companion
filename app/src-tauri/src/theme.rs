//! The Windows light/dark settings: the tray icon follows the taskbar, the window's first paint
//! follows the apps setting. Elsewhere macOS tints the tray icon itself, and Linux panels are
//! usually dark.

#[cfg(windows)]
fn personalize(name: &str) -> Option<u32> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .ok()?
        .get_value(name)
        .ok()
}

#[cfg(not(windows))]
fn personalize(_name: &str) -> Option<u32> {
    None
}

pub fn light_taskbar() -> bool {
    personalize("SystemUsesLightTheme") == Some(1)
}

pub fn light_apps() -> bool {
    personalize("AppsUseLightTheme") != Some(0)
}
