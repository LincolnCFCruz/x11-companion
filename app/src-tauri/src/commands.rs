//! Commands the settings window invokes. They return the same JSON shapes the web UI has always
//! used, and errors as `{error, reason}`.

use attack_shark_x11::settings::{
    ButtonsPatch, DpiPatch, LightingPatch, apply_buttons, apply_dpi, apply_lighting, apply_polling_rate, apply_profile,
    read_state,
};
use attack_shark_x11::{Device, Error};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, State};

use crate::worker::{LiveView, Worker};

#[derive(Debug, Serialize)]
pub struct ApiError {
    error: String,
    /// `disconnected`, `asleep`, `device` or `invalid`.
    reason: &'static str,
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        let reason = match error {
            Error::NotFound => "disconnected",
            Error::NoResponse(_) => "asleep",
            Error::Device(_) => "device",
            Error::Invalid(_) | Error::Guard(_) => "invalid",
        };
        Self { error: error.to_string(), reason }
    }
}

type Reply = Result<Value, ApiError>;

#[tauri::command]
pub async fn get_state(worker: State<'_, Worker>) -> Reply {
    Ok(worker.run(read_state::<Device>).await?)
}

#[tauri::command]
pub fn get_live(worker: State<'_, Worker>) -> LiveView {
    worker.live(crate::startup::is_enabled())
}

#[tauri::command]
pub async fn update_dpi(worker: State<'_, Worker>, patch: DpiPatch) -> Reply {
    Ok(worker.run(move |mouse| apply_dpi(mouse, None, &patch).map(|dpi| json!({ "dpi": dpi }))).await?)
}

#[derive(Debug, Deserialize)]
pub struct PollingRatePatch {
    hz: u32,
}

#[tauri::command]
pub async fn update_polling_rate(worker: State<'_, Worker>, patch: PollingRatePatch) -> Reply {
    let job = move |mouse: &mut Device| {
        apply_polling_rate(mouse, None, Some(patch.hz)).map(|hz| json!({ "polling_rate": hz }))
    };
    Ok(worker.run(job).await?)
}

#[tauri::command]
pub async fn update_lighting(worker: State<'_, Worker>, patch: LightingPatch) -> Reply {
    let job =
        move |mouse: &mut Device| apply_lighting(mouse, None, &patch).map(|lighting| json!({ "lighting": lighting }));
    Ok(worker.run(job).await?)
}

#[tauri::command]
pub async fn update_buttons(worker: State<'_, Worker>, patch: ButtonsPatch) -> Reply {
    let job = move |mouse: &mut Device| {
        apply_buttons(mouse, None, &patch, false).map(|buttons| json!({ "buttons": buttons }))
    };
    Ok(worker.run(job).await?)
}

#[derive(Debug, Deserialize)]
pub struct ProfilePatch {
    active: u32,
}

/// Switch profile, then return the whole state: everything else belongs to the new profile.
#[tauri::command]
pub async fn update_profile(worker: State<'_, Worker>, patch: ProfilePatch) -> Reply {
    let job = move |mouse: &mut Device| {
        apply_profile(mouse, Some(patch.active), false)?;
        read_state(mouse)
    };
    Ok(worker.run(job).await?)
}

#[tauri::command]
pub fn set_startup(app: AppHandle, enabled: bool) -> Reply {
    crate::startup::set_enabled(enabled).map_err(|error| ApiError {
        error: format!("Couldn't change the startup setting: {error}"),
        reason: "device",
    })?;
    crate::tray::sync_startup(&app);
    Ok(json!({ "startup": crate::startup::is_enabled() }))
}
