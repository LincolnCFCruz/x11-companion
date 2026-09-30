//! The device thread.
//!
//! One long-lived thread owns the mouse: Windows cancels a thread's pending HID reads when the
//! thread exits, so device calls can't run on short-lived async tasks. Commands send it jobs;
//! while idle it keeps reading events and reconnects after the dongle is unplugged. It pushes a
//! `live` snapshot to the window and the tray on every change, and at least every 2 s so the
//! battery reading's age stays current.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use attack_shark_x11::protocol::{Battery, Event};
use attack_shark_x11::{Device, Error};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

const IDLE_POLL: Duration = Duration::from_millis(500);
const EMIT_EVERY: Duration = Duration::from_secs(2);

type Job = Box<dyn FnOnce(&mut Device) -> Result<Value, Error> + Send>;
type Reply = oneshot::Sender<Result<Value, Error>>;

/// What the window and tray show between settings reads.
#[derive(Debug, Default)]
struct Live {
    connection: Option<&'static str>,
    battery: Option<(Battery, Instant)>,
    stage: Counter,
    profile: Counter,
    changed: bool,
}

/// A value from the mouse and how many times it changed, so the window can tell a new DPI-button
/// press from one it has already seen.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct Counter {
    seq: u64,
    value: Option<u8>,
}

impl Counter {
    fn bump(&mut self, value: u8) {
        self.seq += 1;
        self.value = Some(value);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LiveView {
    pub connected: bool,
    pub connection: Option<&'static str>,
    pub battery: Option<BatteryView>,
    pub stage: Counter,
    pub profile: Counter,
    /// Whether the app starts at login; `None` when that can't be read.
    pub startup: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatteryView {
    pub level: u8,
    pub state: &'static str,
    /// Seconds since the report.
    pub age: f64,
    #[serde(skip)]
    pub reading: Battery,
}

#[derive(Debug)]
pub struct Worker {
    jobs: Sender<(Job, Reply)>,
    live: Arc<Mutex<Live>>,
}

fn lock(live: &Mutex<Live>) -> MutexGuard<'_, Live> {
    live.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Worker {
    pub fn start(app: AppHandle) -> Self {
        let (jobs, inbox) = mpsc::channel();
        let live = Arc::new(Mutex::new(Live::default()));
        let shared = live.clone();
        thread::Builder::new()
            .name("x11-device".into())
            .spawn(move || device_thread(&app, &inbox, &shared))
            .expect("start the device thread");
        Self { jobs, live }
    }

    /// Run `job` with the mouse on the device thread.
    pub async fn run<T: Serialize>(
        &self,
        job: impl FnOnce(&mut Device) -> Result<T, Error> + Send + 'static,
    ) -> Result<Value, Error> {
        let (reply, answer) = oneshot::channel();
        let job: Job =
            Box::new(move |device| job(device).map(|value| serde_json::to_value(value).expect("views serialize")));
        let stopped = || Error::Device("the device thread stopped".into());
        self.jobs.send((job, reply)).map_err(|_| stopped())?;
        answer.await.map_err(|_| stopped())?
    }

    pub fn live(&self, startup: Option<bool>) -> LiveView {
        snapshot(&lock(&self.live), startup)
    }
}

fn snapshot(live: &Live, startup: Option<bool>) -> LiveView {
    LiveView {
        connected: live.connection.is_some(),
        connection: live.connection,
        battery: live.battery.map(|(reading, at)| BatteryView {
            level: reading.level,
            state: reading.state().map_or("unknown", |state| state.slug()),
            age: (at.elapsed().as_secs_f64() * 10.0).round() / 10.0,
            reading,
        }),
        stage: live.stage,
        profile: live.profile,
        startup,
    }
}

fn record(live: &Mutex<Live>, event: &Event) {
    let mut live = lock(live);
    match *event {
        Event::Battery(battery) => live.battery = Some((battery, Instant::now())),
        Event::DpiStage(stage) => live.stage.bump(stage),
        Event::Profile(profile) => live.profile.bump(profile),
        Event::Ack { .. } => return,
    }
    live.changed = true;
}

/// Run `job` with the mouse, opening it first if needed and dropping it after a device error
/// (unplugged, most likely) so the next call reopens it.
fn with_device<T>(
    device: &mut Option<Device>,
    live: &Arc<Mutex<Live>>,
    job: impl FnOnce(&mut Device) -> Result<T, Error>,
) -> Result<T, Error> {
    if device.is_none() {
        match Device::open() {
            Ok(mut opened) => {
                let hook = live.clone();
                opened.set_on_event(move |event| record(&hook, event));
                let mut state = lock(live);
                state.connection = Some(opened.connection());
                state.changed = true;
                *device = Some(opened);
            }
            Err(error) => {
                let mut state = lock(live);
                if state.connection.is_some() {
                    *state = Live { changed: true, ..Live::default() };
                }
                return Err(error);
            }
        }
    }
    let result = job(device.as_mut().expect("opened above"));
    if let Err(Error::Device(_)) = result {
        *device = None;
        let mut state = lock(live);
        *state = Live { changed: true, ..Live::default() };
    }
    result
}

fn device_thread(app: &AppHandle, inbox: &Receiver<(Job, Reply)>, live: &Arc<Mutex<Live>>) {
    let mut device = None;
    let mut last_emit: Option<Instant> = None;
    loop {
        match inbox.recv_timeout(IDLE_POLL) {
            Ok((job, reply)) => {
                let _ = reply.send(with_device(&mut device, live, job));
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = with_device(&mut device, live, |mouse| mouse.pending_events());
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let due = last_emit.is_none_or(|at| at.elapsed() >= EMIT_EVERY);
        let view = {
            let mut state = lock(live);
            if !state.changed && !due {
                continue;
            }
            state.changed = false;
            snapshot(&state, crate::startup::is_enabled())
        };
        last_emit = Some(Instant::now());
        let _ = app.emit("live", &view);
        crate::tray::update(app, &view);
    }
}
