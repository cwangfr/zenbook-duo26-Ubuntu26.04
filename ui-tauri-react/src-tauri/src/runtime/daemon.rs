use crate::runtime::display_safety::{DisplayScope, DockSource};
use std::fs;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use chrono::{Duration as ChronoDuration, Utc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, RwLock};
use tokio::time::{timeout, Duration, Instant};

use crate::ipc::protocol::{
    DaemonRequest, DaemonResponse, Envelope, LifecyclePhase, SessionCommand, SessionResponse,
    PROTOCOL_VERSION,
};
use crate::runtime::{logger, paths, state::RuntimeState};
use crate::{
    commands, hardware,
    models::{DisplayLayout, EventCategory, HardwareEvent, Orientation},
};

const SESSION_COMMAND_TIMEOUT_DEFAULT: Duration = Duration::from_secs(3);
const SESSION_COMMAND_TIMEOUT_DOCK_MODE: Duration = Duration::from_secs(12);
const SESSION_COMMAND_TIMEOUT_PROBE: Duration = Duration::from_millis(1200);
const SESSION_TIMEOUT_DISCONNECT_THRESHOLD: usize = 3;
#[cfg(test)]
const DOCK_REPLAY_MAX_ATTEMPTS: usize = 3;
const STARTUP_GRACE_WINDOW: Duration = Duration::from_secs(5);
// Mutter can spend several seconds redrawing after s2idle. This only suppresses
// the non-modeset layout probe. ponytail: 15s ceiling; the agent re-registers
// every 5s after that. Upgrade path is an explicit session-ready signal.
const RESUME_GRACE_WINDOW: Duration = Duration::from_secs(15);

static SESSION_GENERATION: AtomicU64 = AtomicU64::new(0);
static SESSION_TIMEOUT_STREAK: AtomicUsize = AtomicUsize::new(0);
static STARTUP_REPLAY_STATE: OnceLock<Mutex<StartupReplayState>> = OnceLock::new();
static RESUME_GRACE_UNTIL: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

#[derive(Debug)]
struct StartupReplayState {
    grace_until: Option<Instant>,
    replay_pending: bool,
    replay_done: bool,
    replay_in_progress: bool,
}

impl Default for StartupReplayState {
    fn default() -> Self {
        Self {
            grace_until: None,
            replay_pending: false,
            replay_done: false,
            replay_in_progress: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupReplayDecision {
    Start,
    InProgress,
    NotPending,
}

pub async fn run() -> Result<(), String> {
    crate::runtime::runtime_dir::ensure_target_user_runtime_dir()?;
    ensure_parent(paths::daemon_socket_path().as_path())
        .map_err(|e| format!("Failed to prepare daemon runtime dir: {e}"))?;
    remove_stale_socket(paths::daemon_socket_path().as_path());

    let listener = UnixListener::bind(paths::daemon_socket_path())
        .map_err(|e| format!("Failed to bind daemon socket: {e}"))?;
    configure_daemon_socket(paths::daemon_socket_path().as_path())
        .map_err(|e| format!("Failed to configure daemon socket: {e}"))?;
    let state = Arc::new(RwLock::new(initialize_state()));
    crate::runtime::monitor::start(state.clone());
    crate::runtime::logind::start(state.clone());
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            cleanup_display_work(&cleanup_state).await;
        }
    });

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("Failed to accept daemon client: {e}"))?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_client(stream, state).await {
                log::warn!("daemon client error: {err}");
            }
        });
    }
}

fn configure_daemon_socket(path: &Path) -> Result<(), String> {
    let uid = std::env::var("ZENBOOK_DUO_UID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok());
    let gid = std::env::var("ZENBOOK_DUO_GID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok());

    if let (Some(uid), Some(gid)) = (uid, gid) {
        let path_cstr = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| format!("Socket path contains interior NUL: {}", path.display()))?;
        let result = unsafe { libc::chown(path_cstr.as_ptr(), uid, gid) };
        if result != 0 {
            return Err(format!(
                "chown({}, {}, {}) failed: {}",
                path.display(),
                uid,
                gid,
                io::Error::last_os_error()
            ));
        }
    }

    fs::set_permissions(path, fs::Permissions::from_mode(0o660))
        .map_err(|e| format!("chmod {} failed: {e}", path.display()))?;

    Ok(())
}

fn initialize_state() -> RuntimeState {
    let storage = Arc::new(crate::runtime::store::RuntimeStore::new(
        paths::state_file_path(),
    ));
    let mut state =
        RuntimeState::load_from(storage.clone()).unwrap_or_else(|reason| RuntimeState {
            storage,
            storage_fault: Some(reason),
            ..RuntimeState::default()
        });
    state.status = crate::runtime::probe::current_status();
    state.status.service_active = false;
    state.settings = commands::settings::load_settings_local();
    let charge_limit = state.settings.charge_limit_percent;
    match hardware::battery::set_charge_limit(charge_limit) {
        Ok(_) => {
            let _ = logger::append_line(format!(
                "rust-daemon: restored battery charge limit -> {charge_limit}%"
            ));
        }
        Err(error) => {
            let _ = logger::append_line(format!(
                "rust-daemon: battery charge limit restore unavailable: {error}"
            ));
        }
    }
    match (
        crate::runtime::state::current_boot_id(),
        crate::runtime::display_safety::new_identity(),
    ) {
        (Some(boot), Ok(id)) => state.display_safety.begin_daemon(&boot, id),
        _ => state.storage_fault = Some("Cannot establish display safety identity".into()),
    }
    state.session_agent = Default::default();
    state.validate_secondary_panel_ownership_for_current_boot();
    state.touch();
    persist_state(&state);
    let _ = logger::append_line("rust-daemon: initialized runtime state");
    state
}

fn startup_replay_state_cell() -> &'static Mutex<StartupReplayState> {
    STARTUP_REPLAY_STATE.get_or_init(|| Mutex::new(StartupReplayState::default()))
}

async fn mark_startup_replay_pending_from_boot() {
    let mut guard = startup_replay_state_cell().lock().await;
    guard.replay_pending = true;
    guard.replay_done = false;
    guard.replay_in_progress = false;
    guard.grace_until = None;
}

async fn startup_replay_decision_on_registration() -> StartupReplayDecision {
    let mut guard = startup_replay_state_cell().lock().await;
    if guard.replay_in_progress {
        return StartupReplayDecision::InProgress;
    }
    if !guard.replay_pending || guard.replay_done {
        return StartupReplayDecision::NotPending;
    }

    guard.replay_in_progress = true;
    guard.grace_until = Some(Instant::now() + STARTUP_GRACE_WINDOW);
    StartupReplayDecision::Start
}

async fn finish_startup_replay() {
    let mut guard = startup_replay_state_cell().lock().await;
    guard.replay_pending = false;
    guard.replay_done = true;
    guard.replay_in_progress = false;
    guard.grace_until = None;
}

async fn startup_grace_active_for_display_probe() -> bool {
    let now = Instant::now();
    let guard = startup_replay_state_cell().lock().await;
    guard.replay_in_progress
        && !guard.replay_done
        && guard
            .grace_until
            .map(|deadline| now < deadline)
            .unwrap_or(false)
}

fn resume_grace_cell() -> &'static Mutex<Option<Instant>> {
    RESUME_GRACE_UNTIL.get_or_init(|| Mutex::new(None))
}

async fn note_resume_grace() {
    *resume_grace_cell().lock().await = Some(Instant::now() + RESUME_GRACE_WINDOW);
}

pub(crate) async fn resume_grace_active() -> bool {
    let now = Instant::now();
    resume_grace_cell()
        .lock()
        .await
        .map(|deadline| now < deadline)
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) async fn reset_resume_grace_for_test() {
    *resume_grace_cell().lock().await = None;
}

async fn display_probe_grace_active() -> bool {
    startup_grace_active_for_display_probe().await || resume_grace_active().await
}

async fn suppress_layout_probe_disconnect(state: &Arc<RwLock<RuntimeState>>) -> bool {
    // Sampled at timeout, not at request start. A probe already in flight when
    // suspend begins has neither grace nor the suspended flag yet.
    display_probe_grace_active().await || state.read().await.display_safety.suspended
}

pub(crate) async fn run_startup_replay_once(state: Arc<RwLock<RuntimeState>>) {
    let _ = super::dock_transaction::submit_current_dock_target(&state, DockSource::Startup).await;
    finish_startup_replay().await;
}

async fn handle_client(stream: UnixStream, state: Arc<RwLock<RuntimeState>>) -> Result<(), String> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| format!("Failed to read daemon request: {e}"))?
    {
        let envelope: Envelope<DaemonRequest> =
            serde_json::from_str(&line).map_err(|e| format!("Invalid daemon request JSON: {e}"))?;

        if envelope.protocol_version != PROTOCOL_VERSION {
            write_response(
                &mut writer,
                DaemonResponse::Error {
                    message: format!(
                        "Protocol mismatch: expected {}, got {}",
                        PROTOCOL_VERSION, envelope.protocol_version
                    ),
                },
            )
            .await?;
            continue;
        }

        let response = match envelope.payload {
            DaemonRequest::Ping => DaemonResponse::Pong,
            DaemonRequest::HandleLifecycle { phase } => match handle_lifecycle(&state, phase).await
            {
                Ok(()) => DaemonResponse::Ack,
                Err(message) => DaemonResponse::Error { message },
            },
            DaemonRequest::GetStatus => {
                let guard = state.read().await;
                let mut status = guard.status.clone();
                status.service_active = guard.session_agent.connected;
                DaemonResponse::Status { status }
            }
            DaemonRequest::GetBatteryStatus => {
                let configured = state.read().await.settings.charge_limit_percent;
                DaemonResponse::BatteryStatus {
                    status: hardware::battery::read_status(configured),
                }
            }
            DaemonRequest::GetDisplayLayout => {
                match request_session(state.clone(), SessionCommand::GetDisplayLayout, true).await {
                    Ok(SessionResponse::DisplayLayout { layout }) => {
                        DaemonResponse::DisplayLayout { layout }
                    }
                    Ok(SessionResponse::Error { message }) => DaemonResponse::Error { message },
                    Ok(_) => DaemonResponse::Error {
                        message: "Unexpected session-agent response while reading display layout"
                            .into(),
                    },
                    Err(_) => match hardware::display_config::get_display_layout() {
                        Ok(layout) => DaemonResponse::DisplayLayout { layout },
                        Err(message) => DaemonResponse::Error { message },
                    },
                }
            }
            DaemonRequest::GetSettings => {
                let guard = state.read().await;
                DaemonResponse::Settings {
                    settings: guard.settings.clone(),
                }
            }
            DaemonRequest::UsbMediaRemapStatus => DaemonResponse::UsbMediaRemapStatus {
                status: commands::usb_media_remap::get_status(),
            },
            DaemonRequest::SaveSettings { settings } => {
                let mut guard = state.write().await;
                guard.settings = settings;
                guard.touch();
                persist_state(&guard);
                DaemonResponse::Ack
            }
            DaemonRequest::SetBacklight { level } => match hardware::hid::set_backlight(level) {
                Ok(()) => {
                    let mut guard = state.write().await;
                    guard.status.backlight_level = level;
                    let _ = logger::append_line(format!(
                        "rust-daemon: set backlight request -> {level}"
                    ));
                    guard.recent_events.push(HardwareEvent::info(
                        EventCategory::Keyboard,
                        format!("Backlight set to {level}"),
                        "rust-daemon",
                    ));
                    guard.touch();
                    persist_state(&guard);
                    DaemonResponse::Ack
                }
                Err(message) => DaemonResponse::Error { message },
            },
            DaemonRequest::SetDisplayBrightness { percent } => {
                match hardware::sysfs::set_display_brightness_percent(percent) {
                    Ok(()) => {
                        let mut guard = state.write().await;
                        guard.settings.last_display_brightness_percent = Some(percent);
                        guard.status.display_brightness =
                            hardware::sysfs::read_display_brightness();
                        match commands::settings::save_settings_local(guard.settings.clone()) {
                            Ok(()) => {
                                guard.touch();
                                persist_state(&guard);
                                DaemonResponse::Ack
                            }
                            Err(message) => DaemonResponse::Error { message },
                        }
                    }
                    Err(message) => DaemonResponse::Error { message },
                }
            }
            DaemonRequest::SetChargeLimit { limit } => {
                match hardware::battery::set_charge_limit(limit) {
                    Ok(status) => {
                        let mut guard = state.write().await;
                        guard.settings.charge_limit_percent = limit;
                        match commands::settings::save_settings_local(guard.settings.clone()) {
                            Ok(()) => {
                                let _ = logger::append_line(format!(
                                    "rust-daemon: set battery charge limit -> {limit}%"
                                ));
                                guard.touch();
                                persist_state(&guard);
                                DaemonResponse::BatteryStatus { status }
                            }
                            Err(message) => DaemonResponse::Error {
                                message: format!(
                                    "The {limit}% limit is active, but saving it failed: {message}"
                                ),
                            },
                        }
                    }
                    Err(message) => DaemonResponse::Error { message },
                }
            }
            DaemonRequest::ApplyPerformanceMode { mode } => {
                let mut guard = state.write().await;
                let limits = guard.settings.performance_profiles.for_mode(&mode).clone();
                match hardware::power::apply_performance_mode(&mode, &limits) {
                    Ok(()) => {
                        guard.settings.active_performance_mode = mode.clone();
                        guard.battery_saver_restore_mode = None;
                        match commands::settings::save_settings_local(guard.settings.clone()) {
                            Ok(()) => {
                                guard.touch();
                                persist_state(&guard);
                                DaemonResponse::Ack
                            }
                            Err(message) => DaemonResponse::Error { message },
                        }
                    }
                    Err(message) => DaemonResponse::Error { message },
                }
            }
            DaemonRequest::ApplySensorOrientation { scope, orientation } => {
                match super::dock_transaction::apply_sensor(&state, scope, orientation).await {
                    Ok(_) => DaemonResponse::Ack,
                    Err(message) => DaemonResponse::Error { message },
                }
            }
            DaemonRequest::SetOrientation { orientation } => {
                apply_orientation(&state, orientation).await
            }
            DaemonRequest::ApplyDisplayLayout { layout } => {
                apply_display_layout_request(&state, layout).await
            }
            DaemonRequest::UsbMediaRemapStart => {
                let _ = logger::append_line("rust-daemon: start usb media remap request");
                match commands::usb_media_remap::start_remap() {
                    Ok(()) => DaemonResponse::Ack,
                    Err(message) => DaemonResponse::Error { message },
                }
            }
            DaemonRequest::UsbMediaRemapStop => {
                let _ = logger::append_line("rust-daemon: stop usb media remap request");
                match commands::usb_media_remap::stop_remap() {
                    Ok(()) => DaemonResponse::Ack,
                    Err(message) => DaemonResponse::Error { message },
                }
            }
            DaemonRequest::UsbMediaRemapTogglePause => {
                let _ = logger::append_line("rust-daemon: toggle usb media remap pause request");
                match commands::usb_media_remap::toggle_pause() {
                    Ok(()) => DaemonResponse::Ack,
                    Err(message) => DaemonResponse::Error { message },
                }
            }
            DaemonRequest::RestartService => DaemonResponse::Error {
                message: "Service restart not yet owned by rust-daemon".into(),
            },
            DaemonRequest::RegisterSessionAgent {
                agent_id,
                session_id,
                backend,
                socket_path,
            } => {
                match handle_session_registration(
                    &state,
                    session_id,
                    backend,
                    socket_path,
                    agent_id,
                )
                .await
                {
                    Ok(scope) => {
                        let g = state.read().await;
                        DaemonResponse::SessionRegistered {
                            scope,
                            automatic_blocked: g.display_safety.degraded.is_some()
                                || g.display_safety.suspended
                                || g.storage_fault.is_some(),
                            manual_allowed: g.display_safety.manual_allowed
                                && g.display_safety.pending.is_none()
                                && !g.display_safety.manual_pending
                                && !g.display_safety.suspended
                                && g.storage_fault.is_none(),
                        }
                    }
                    Err(message) => DaemonResponse::Error { message },
                }
            }

            DaemonRequest::TailLogs { lines } => DaemonResponse::Logs {
                lines: hardware::sysfs::read_log_lines(lines),
            },
            DaemonRequest::ClearLogs => match logger::clear() {
                Ok(()) => DaemonResponse::Ack,
                Err(message) => DaemonResponse::Error { message },
            },
            DaemonRequest::GetRecentEvents { limit } => {
                let guard = state.read().await;
                let events = guard
                    .recent_events
                    .iter()
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                DaemonResponse::Events { events }
            }
            DaemonRequest::ListTouchscreens => {
                let devices = hardware::touchscreen::list_touchscreens();
                DaemonResponse::Touchscreens { devices }
            }
            DaemonRequest::SetTouchscreenEnabled { connector, enabled } => {
                let devices = hardware::touchscreen::list_touchscreens();
                match devices.iter().find(|d| d.connector == connector) {
                    Some(dev) => {
                        match hardware::touchscreen::set_touchscreen_enabled(&dev.i2c_id, enabled) {
                            Ok(()) => DaemonResponse::Ack,
                            Err(message) => DaemonResponse::Error { message },
                        }
                    }
                    None => DaemonResponse::Error {
                        message: format!("No touchscreen found for connector {}", connector),
                    },
                }
            }
        };

        write_response(&mut writer, response).await?;
    }

    Ok(())
}

async fn write_response<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    response: DaemonResponse,
) -> Result<(), String> {
    let line = serde_json::to_string(&Envelope::new(response))
        .map_err(|e| format!("Failed to encode daemon response: {e}"))?;
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("Failed to write daemon response: {e}"))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|e| format!("Failed to terminate daemon response: {e}"))
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn remove_stale_socket(path: &Path) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_socket() {
            let _ = fs::remove_file(path);
        }
    }
}

fn persist_state(state: &RuntimeState) {
    if let Err(err) = state.save() {
        log::warn!("failed to persist runtime state: {err}");
        let _ = logger::append_line(format!("rust-daemon: failed to persist state: {err}"));
    }
}

async fn handle_lifecycle(
    state: &Arc<RwLock<RuntimeState>>,
    phase: LifecyclePhase,
) -> Result<(), String> {
    let restore = matches!(
        phase,
        LifecyclePhase::Post | LifecyclePhase::Thaw | LifecyclePhase::Boot
    );
    let result = handle_lifecycle_with(
        state,
        phase,
        crate::runtime::probe::keyboard_attached(),
        hardware::hid::set_backlight,
    )
    .await;
    if restore {
        let disabled = state.read().await.settings.touchscreen_disabled.clone();
        tokio::task::spawn_blocking(move || {
            for connector in disabled {
                if let Some(dev) = hardware::touchscreen::list_touchscreens()
                    .iter()
                    .find(|d| d.connector == connector)
                {
                    let _ = hardware::touchscreen::set_touchscreen_enabled(&dev.i2c_id, false);
                }
            }
        });
    }
    result
}

pub(crate) async fn cleanup_display_work(state: &Arc<RwLock<RuntimeState>>) {
    let cleanup = {
        let g = state.read().await;
        !g.display_safety.worker_running
            && (g.display_safety.pending.is_some() || g.display_safety.receipt_to_consume.is_some())
    };
    if cleanup {
        let _ = super::dock_transaction::reconcile_pending_dock(
            state,
            super::dock_transaction::ReconcileMode::LateCleanup,
        )
        .await;
    }
    super::dock_transaction::resume_dock_target(state).await;
}

pub(crate) async fn handle_session_registration(
    state: &Arc<RwLock<RuntimeState>>,
    session_id: String,
    backend: crate::ipc::protocol::SessionBackend,
    socket_path: String,
    agent_id: String,
) -> Result<DisplayScope, String> {
    if agent_id.is_empty() {
        return Err("Agent identity is required".into());
    }
    let same_agent = {
        let g = state.read().await;
        g.session_agent.connected
            && g.session_agent.socket_path.as_deref() == Some(socket_path.as_str())
            && g.session_agent
                .scope
                .as_ref()
                .is_some_and(|scope| scope.agent_id == agent_id)
    };
    if same_agent {
        let refreshed = {
            let mut g = state.write().await;
            let still_same = g.session_agent.connected
                && g.session_agent.socket_path.as_deref() == Some(socket_path.as_str())
                && g.session_agent
                    .scope
                    .as_ref()
                    .is_some_and(|scope| scope.agent_id == agent_id);
            if still_same {
                g.session_agent.session_id = Some(session_id.clone());
                g.session_agent.backend = Some(backend.clone());
                g.status.service_active = true;
                let scope = g
                    .session_agent
                    .scope
                    .clone()
                    .ok_or("No verified display scope")?;
                g.touch();
                g.save()?;
                Some(scope)
            } else {
                None
            }
        };
        if let Some(scope) = refreshed {
            let state = state.clone();
            tokio::spawn(async move {
                let _ = super::dock_transaction::bind_current_context(&state).await;
            });
            return Ok(scope);
        }
    }
    let scope = {
        let mut g = state.write().await;
        let scope = DisplayScope {
            boot_id: g.display_safety.boot_id.clone(),
            daemon_id: g.display_safety.daemon_id.clone(),
            agent_id,
            registration_generation: SESSION_GENERATION.fetch_add(1, Ordering::Relaxed) + 1,
            lifecycle_epoch: g.display_safety.lifecycle_epoch,
        };
        if g.display_safety.pending.is_some() || g.display_safety.manual_pending {
            g.display_safety.degraded =
                Some("Registration changed while a display operation was pending".into());
            g.display_safety.manual_allowed = false;
        }
        g.session_agent.connected = true;
        g.session_agent.session_id = Some(session_id);
        g.session_agent.backend = Some(backend);
        g.session_agent.socket_path = Some(socket_path);
        g.session_agent.scope = Some(scope.clone());
        g.status.service_active = true;
        g.touch();
        g.save()?;
        scope
    };
    let decision = startup_replay_decision_on_registration().await;
    let state = state.clone();
    tokio::spawn(async move {
        let _ = super::dock_transaction::bind_current_context(&state).await;
        if decision == StartupReplayDecision::Start {
            run_startup_replay_once(state).await;
        } else if decision == StartupReplayDecision::NotPending {
            let _ = super::dock_transaction::submit_current_dock_target(
                &state,
                DockSource::Registration,
            )
            .await;
        }
    });
    Ok(scope)
}
async fn replay_current_dock_mode(
    state: &Arc<RwLock<RuntimeState>>,
    attached: bool,
    scale: f64,
) -> Result<(), String> {
    super::dock_transaction::submit_dock_target(state, attached, scale, DockSource::Resume)
        .await
        .map(|_| ())
}

#[cfg(test)]
fn dock_replay_attempt_limit(attached: bool) -> usize {
    if attached {
        DOCK_REPLAY_MAX_ATTEMPTS
    } else {
        // A bad xe eDP-2 pipe can block a detached modeset for several seconds.
        // Never multiply that stall by retrying the same failing request.
        1
    }
}

pub(crate) async fn forward_or_queue_dock_mode(
    state: &Arc<RwLock<RuntimeState>>,
    attached: bool,
    scale: f64,
) -> Result<(), String> {
    super::dock_transaction::submit_dock_target(state, attached, scale, DockSource::Monitor)
        .await
        .map(|_| ())
}

pub(crate) async fn session_display_layout(
    state: Arc<RwLock<RuntimeState>>,
) -> Option<DisplayLayout> {
    match request_session(state, SessionCommand::GetDisplayLayout, true).await {
        Ok(SessionResponse::DisplayLayout { layout }) => Some(layout),
        _ => None,
    }
}

pub(crate) async fn apply_orientation(
    state: &Arc<RwLock<RuntimeState>>,
    orientation: Orientation,
) -> DaemonResponse {
    let scale = state.read().await.settings.default_scale;
    match super::dock_transaction::apply_manual(
        state,
        crate::ipc::protocol::ManualDisplayMutation::Orientation {
            orientation: orientation.clone(),
            scale,
        },
    )
    .await
    {
        Ok(_) => {
            let mut g = state.write().await;
            g.status.orientation = orientation;
            g.touch();
            match g.save() {
                Ok(()) => DaemonResponse::Ack,
                Err(message) => DaemonResponse::Error { message },
            }
        }
        Err(message) => DaemonResponse::Error { message },
    }
}
async fn apply_display_layout_request(
    state: &Arc<RwLock<RuntimeState>>,
    layout: DisplayLayout,
) -> DaemonResponse {
    match super::dock_transaction::apply_manual(
        state,
        crate::ipc::protocol::ManualDisplayMutation::Layout {
            layout: layout.clone(),
        },
    )
    .await
    {
        Ok(_) => {
            let mut g = state.write().await;
            crate::runtime::probe::apply_layout_to_status(&mut g.status, Some(&layout));
            g.touch();
            match g.save() {
                Ok(()) => DaemonResponse::Ack,
                Err(message) => DaemonResponse::Error { message },
            }
        }
        Err(message) => DaemonResponse::Error { message },
    }
}

pub(crate) async fn request_session(
    state: Arc<RwLock<RuntimeState>>,
    command: SessionCommand,
    disconnect_on_failure: bool,
) -> Result<SessionResponse, String> {
    let is_dock_mode_command = matches!(command, SessionCommand::SetDockMode { .. });
    let is_display_probe_command = matches!(command, SessionCommand::GetDisplayLayout);
    let timeout_for_command = session_timeout_for_command(&command);
    let disconnect_on_timeout = disconnect_on_failure && should_disconnect_on_timeout(&command);

    let socket_path = {
        let guard = state.read().await;
        guard
            .session_agent
            .socket_path
            .clone()
            .ok_or_else(|| "No session agent registered".to_string())?
    };

    let stream = match timeout(timeout_for_command, UnixStream::connect(&socket_path)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            if disconnect_on_failure {
                mark_session_agent_disconnected(
                    &state,
                    &format!("Failed to connect to session agent: {e}"),
                )
                .await;
            }
            return Err(format!("Failed to connect to session agent: {e}"));
        }
        Err(_) => {
            let suppress =
                is_display_probe_command && suppress_layout_probe_disconnect(&state).await;
            if disconnect_on_timeout && !suppress {
                mark_session_agent_disconnected(&state, "Timed out connecting to session agent")
                    .await;
            } else if disconnect_on_timeout && suppress {
                let _ = logger::append_line(
                    "rust-daemon: layout probe timeout kept session agent connected".to_string(),
                );
            }
            return Err("Timed out connecting to session agent".to_string());
        }
    };
    let (reader, mut writer) = stream.into_split();

    let line = serde_json::to_string(&Envelope::new(command))
        .map_err(|e| format!("Failed to encode session command: {e}"))?;
    if let Err(e) = timeout(timeout_for_command, writer.write_all(line.as_bytes()))
        .await
        .map_err(|_| "Timed out writing session command".to_string())?
    {
        if disconnect_on_failure {
            mark_session_agent_disconnected(
                &state,
                &format!("Failed to write session command: {e}"),
            )
            .await;
        }
        return Err(format!("Failed to write session command: {e}"));
    }
    if let Err(e) = timeout(timeout_for_command, writer.write_all(b"\n"))
        .await
        .map_err(|_| "Timed out terminating session command".to_string())?
    {
        if disconnect_on_failure {
            mark_session_agent_disconnected(
                &state,
                &format!("Failed to terminate session command: {e}"),
            )
            .await;
        }
        return Err(format!("Failed to terminate session command: {e}"));
    }

    let mut lines = BufReader::new(reader).lines();
    let reply = match timeout(timeout_for_command, lines.next_line()).await {
        Err(_) => {
            if is_dock_mode_command && disconnect_on_failure {
                let probe_ok = probe_session_agent(socket_path.as_str()).await;
                if probe_ok {
                    SESSION_TIMEOUT_STREAK.store(0, Ordering::Relaxed);
                    let _ = logger::append_line(
                        "rust-daemon: session timeout recovered by probe disconnect_reason_class=timeout_probe_recovered"
                            .to_string(),
                    );
                } else {
                    let streak = SESSION_TIMEOUT_STREAK.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = logger::append_line(format!(
                        "rust-daemon: session timeout probe failed disconnect_reason_class=timeout_unhealthy_probe streak={} threshold={}",
                        streak, SESSION_TIMEOUT_DISCONNECT_THRESHOLD
                    ));
                    if should_disconnect_after_timeout_streak(streak) {
                        mark_session_agent_disconnected(
                            &state,
                            "Timed out waiting for session response (timeout_unhealthy_probe)",
                        )
                        .await;
                        SESSION_TIMEOUT_STREAK.store(0, Ordering::Relaxed);
                    }
                }
            } else {
                let suppress =
                    is_display_probe_command && suppress_layout_probe_disconnect(&state).await;
                if disconnect_on_timeout && !suppress {
                    mark_session_agent_disconnected(
                        &state,
                        "Timed out waiting for session response",
                    )
                    .await;
                } else if disconnect_on_timeout && suppress {
                    let _ = logger::append_line(
                        "rust-daemon: layout probe timeout kept session agent connected"
                            .to_string(),
                    );
                }
            }
            return Err("Timed out waiting for session response".to_string());
        }
        Ok(Ok(Some(reply))) => reply,
        Ok(Ok(None)) => {
            if disconnect_on_failure {
                mark_session_agent_disconnected(&state, "Session agent closed before replying")
                    .await;
            }
            return Err("Session agent closed before replying".to_string());
        }
        Ok(Err(e)) => {
            if disconnect_on_failure {
                mark_session_agent_disconnected(
                    &state,
                    &format!("Failed to read session response: {e}"),
                )
                .await;
            }
            return Err(format!("Failed to read session response: {e}"));
        }
    };

    let envelope: Envelope<SessionResponse> =
        serde_json::from_str(&reply).map_err(|e| format!("Invalid session response JSON: {e}"))?;

    envelope.validate()?;
    SESSION_TIMEOUT_STREAK.store(0, Ordering::Relaxed);
    Ok(envelope.payload)
}

fn session_timeout_for_command(command: &SessionCommand) -> Duration {
    match command {
        SessionCommand::SetDockMode { .. }
        | SessionCommand::ApplyDockOperation { .. }
        | SessionCommand::ApplyDisplayMutation { .. } => SESSION_COMMAND_TIMEOUT_DOCK_MODE,
        _ => SESSION_COMMAND_TIMEOUT_DEFAULT,
    }
}

fn should_disconnect_on_timeout(command: &SessionCommand) -> bool {
    !matches!(command, SessionCommand::SetDockMode { .. })
}

fn should_disconnect_after_timeout_streak(streak: usize) -> bool {
    streak >= SESSION_TIMEOUT_DISCONNECT_THRESHOLD
}

async fn probe_session_agent(socket_path: &str) -> bool {
    let stream = match timeout(
        SESSION_COMMAND_TIMEOUT_PROBE,
        UnixStream::connect(socket_path),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        _ => return false,
    };
    let (reader, mut writer) = stream.into_split();

    let line = match serde_json::to_string(&Envelope::new(SessionCommand::GetDisplayLayout)) {
        Ok(line) => line,
        Err(_) => return false,
    };

    if timeout(
        SESSION_COMMAND_TIMEOUT_PROBE,
        writer.write_all(line.as_bytes()),
    )
    .await
    .is_err()
    {
        return false;
    }
    if writer.write_all(b"\n").await.is_err() {
        return false;
    }

    let mut lines = BufReader::new(reader).lines();
    match timeout(SESSION_COMMAND_TIMEOUT_PROBE, lines.next_line()).await {
        Ok(Ok(Some(reply))) => serde_json::from_str::<Envelope<SessionResponse>>(&reply)
            .is_ok_and(|envelope| envelope.validate().is_ok()),
        _ => false,
    }
}

async fn mark_session_agent_disconnected(state: &Arc<RwLock<RuntimeState>>, reason: &str) {
    let mut guard = state.write().await;
    let was_connected = guard.session_agent.connected || guard.status.service_active;
    guard.session_agent = Default::default();
    guard.status.service_active = false;
    if was_connected {
        guard.recent_events.push(HardwareEvent::warning(
            EventCategory::Service,
            format!("Session agent disconnected: {reason}"),
            "rust-daemon",
        ));
    }
    guard.touch();
    persist_state(&guard);
    let _ = logger::append_line(format!(
        "rust-daemon: session agent disconnected: {}",
        reason
    ));
    if let Err(err) = send_runtime_notification_direct(
        "Zenbook Duo Runtime Error",
        &format!("Session agent disconnected: {reason}"),
    ) {
        log::warn!("failed to send direct disconnect notification: {err}");
    }
}

pub(crate) async fn notify_runtime_error(
    state: &Arc<RwLock<RuntimeState>>,
    title: &str,
    message: &str,
) {
    if !should_notify_runtime_error(title, message) {
        return;
    }

    if !should_emit_runtime_notification(state, title, message).await {
        return;
    }

    if let Err(err) = send_runtime_notification_via_session(state, title, message).await {
        log::warn!("failed to send runtime notification via session agent: {err}");
        if let Err(fallback_err) = send_runtime_notification_direct(title, message) {
            log::warn!("failed to send runtime notification directly: {fallback_err}");
        }
    }
}

fn should_notify_runtime_error(title: &str, message: &str) -> bool {
    if title != "Zenbook Duo Runtime Error" {
        return true;
    }

    message != "Lifecycle dock refresh skipped: No session agent registered"
}

async fn should_emit_runtime_notification(
    state: &Arc<RwLock<RuntimeState>>,
    title: &str,
    message: &str,
) -> bool {
    let key = format!("{title}\n{message}");
    let now = Utc::now();
    let mut guard = state.write().await;
    if let Some(last) = &guard.last_runtime_notification {
        if last.key == key && now - last.emitted_at < ChronoDuration::seconds(30) {
            return false;
        }
    }
    guard.last_runtime_notification = Some(crate::runtime::state::RuntimeNotificationState {
        key,
        emitted_at: now,
    });
    persist_state(&guard);
    true
}

async fn send_runtime_notification_via_session(
    state: &Arc<RwLock<RuntimeState>>,
    title: &str,
    message: &str,
) -> Result<(), String> {
    match request_session(
        state.clone(),
        SessionCommand::ShowNotification {
            title: title.to_string(),
            message: message.to_string(),
            urgent: true,
        },
        false,
    )
    .await?
    {
        SessionResponse::Ack => Ok(()),
        SessionResponse::Error { message } => Err(message),
        _ => Err("Unexpected display-layout response for notification request".into()),
    }
}

fn send_runtime_notification_direct(title: &str, message: &str) -> Result<(), String> {
    let target_user = std::env::var("ZENBOOK_DUO_USER")
        .map_err(|_| "ZENBOOK_DUO_USER is not set for runtime notifications".to_string())?;
    let target_uid = std::env::var("ZENBOOK_DUO_UID")
        .map_err(|_| "ZENBOOK_DUO_UID is not set for runtime notifications".to_string())?;
    let runtime_dir = format!("/run/user/{target_uid}");
    let bus_address = format!("unix:path={runtime_dir}/bus");

    let status = Command::new("sudo")
        .args(["-u", &target_user, "env"])
        .arg(format!("XDG_RUNTIME_DIR={runtime_dir}"))
        .arg(format!("DBUS_SESSION_BUS_ADDRESS={bus_address}"))
        .args([
            "notify-send",
            "-a",
            "Zenbook Duo Control",
            "-u",
            "critical",
            "-i",
            "dialog-error",
            title,
            message,
        ])
        .status()
        .map_err(|e| format!("Failed to launch runtime notification: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("Runtime notification exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    static TEST_SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

    fn test_serial() -> &'static Mutex<()> {
        TEST_SERIAL.get_or_init(|| Mutex::new(()))
    }

    async fn reset_daemon_test_state() {
        *startup_replay_state_cell().lock().await = StartupReplayState::default();
        SESSION_GENERATION.store(0, Ordering::Relaxed);
        SESSION_TIMEOUT_STREAK.store(0, Ordering::Relaxed);
    }

    #[tokio::test]
    async fn runtime_error_notification_forwards_show_notification_command() {
        let _serial = test_serial().lock().await;
        reset_daemon_test_state().await;

        let socket_path = unique_test_socket_path("notify-runtime-error");
        let listener = UnixListener::bind(&socket_path).expect("bind test session socket");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept test session client");
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let line = lines
                .next_line()
                .await
                .expect("read test request")
                .expect("session request line");
            let envelope: Envelope<SessionCommand> =
                serde_json::from_str(&line).expect("decode session request");
            match envelope.payload {
                SessionCommand::ShowNotification {
                    title,
                    message,
                    urgent,
                } => {
                    assert_eq!(title, "Zenbook Duo Runtime Error");
                    assert_eq!(message, "Dock-mode replay skipped");
                    assert!(urgent);
                }
                other => panic!("unexpected session command: {other:?}"),
            }
            let reply =
                serde_json::to_string(&Envelope::new(SessionResponse::Ack)).expect("encode ack");
            writer.write_all(reply.as_bytes()).await.expect("write ack");
            writer.write_all(b"\n").await.expect("terminate ack");
        });

        let state = Arc::new(RwLock::new(crate::runtime::test_utils::state()));
        {
            let mut guard = state.write().await;
            guard.session_agent.connected = true;
            guard.session_agent.socket_path = Some(socket_path.to_string_lossy().into_owned());
        }

        notify_runtime_error(
            &state,
            "Zenbook Duo Runtime Error",
            "Dock-mode replay skipped",
        )
        .await;

        server.await.expect("join session server");
        let _ = fs::remove_file(&socket_path);
    }

    #[tokio::test]
    async fn runtime_error_notification_suppresses_duplicate_messages() {
        let _serial = test_serial().lock().await;
        reset_daemon_test_state().await;

        let socket_path = unique_test_socket_path("notify-runtime-dedupe");
        let listener = UnixListener::bind(&socket_path).expect("bind test session socket");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept first test client");
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let line = lines
                .next_line()
                .await
                .expect("read first request")
                .expect("first session request line");
            let envelope: Envelope<SessionCommand> =
                serde_json::from_str(&line).expect("decode first session request");
            assert!(matches!(
                envelope.payload,
                SessionCommand::ShowNotification { .. }
            ));
            let reply =
                serde_json::to_string(&Envelope::new(SessionResponse::Ack)).expect("encode ack");
            writer.write_all(reply.as_bytes()).await.expect("write ack");
            writer.write_all(b"\n").await.expect("terminate ack");

            timeout(Duration::from_millis(250), listener.accept())
                .await
                .expect_err("duplicate notification should be suppressed");
        });

        let state = Arc::new(RwLock::new(crate::runtime::test_utils::state()));
        {
            let mut guard = state.write().await;
            guard.session_agent.connected = true;
            guard.session_agent.socket_path = Some(socket_path.to_string_lossy().into_owned());
        }

        notify_runtime_error(
            &state,
            "Zenbook Duo Runtime Error",
            "Session agent disconnected",
        )
        .await;
        notify_runtime_error(
            &state,
            "Zenbook Duo Runtime Error",
            "Session agent disconnected",
        )
        .await;

        server.await.expect("join session server");
        let _ = fs::remove_file(&socket_path);
    }

    #[test]
    fn no_session_agent_registered_is_not_notifiable() {
        assert!(!should_notify_runtime_error(
            "Zenbook Duo Runtime Error",
            "Lifecycle dock refresh skipped: No session agent registered",
        ));
    }

    #[test]
    fn real_runtime_errors_remain_notifiable() {
        assert!(should_notify_runtime_error(
            "Zenbook Duo Runtime Error",
            "Dock-mode policy action failed: Timed out waiting for session response",
        ));
    }

    #[test]
    fn set_dock_mode_uses_extended_session_timeout() {
        let timeout = session_timeout_for_command(&SessionCommand::SetDockMode {
            attached: true,
            scale: 1.66,
        });
        assert_eq!(timeout, SESSION_COMMAND_TIMEOUT_DOCK_MODE);
    }

    #[test]
    fn non_dock_commands_keep_default_timeout() {
        let timeout = session_timeout_for_command(&SessionCommand::GetDisplayLayout);
        assert_eq!(timeout, SESSION_COMMAND_TIMEOUT_DEFAULT);
    }

    #[test]
    fn set_dock_mode_timeout_does_not_disconnect_session_agent() {
        assert!(!should_disconnect_on_timeout(
            &SessionCommand::SetDockMode {
                attached: false,
                scale: 1.66,
            }
        ));
        assert!(should_disconnect_on_timeout(
            &SessionCommand::GetDisplayLayout
        ));
    }

    #[test]
    fn disconnect_requires_timeout_streak_threshold() {
        assert!(!should_disconnect_after_timeout_streak(1));
        assert!(!should_disconnect_after_timeout_streak(2));
        assert!(should_disconnect_after_timeout_streak(3));
    }

    #[test]
    fn detached_dock_replay_is_not_retried() {
        assert_eq!(dock_replay_attempt_limit(false), 1);
        assert_eq!(dock_replay_attempt_limit(true), DOCK_REPLAY_MAX_ATTEMPTS);
    }

    fn unique_test_socket_path(label: &str) -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("zenbook-duo-{label}-{nanos}-{id}.sock"))
    }
}

pub(crate) async fn advance_display_epoch(
    state: &Arc<RwLock<RuntimeState>>,
    phase: LifecyclePhase,
) -> Result<(), String> {
    if matches!(phase, LifecyclePhase::Post | LifecyclePhase::Thaw) {
        note_resume_grace().await;
    }
    let registered = {
        let mut g = state.write().await;
        g.display_safety.lifecycle_epoch = g
            .display_safety
            .lifecycle_epoch
            .checked_add(1)
            .ok_or("Lifecycle epoch exhausted")?;
        g.display_safety.suspended = matches!(
            phase,
            LifecyclePhase::Pre | LifecyclePhase::Hibernate | LifecyclePhase::Shutdown
        );
        if matches!(phase, LifecyclePhase::Post | LifecyclePhase::Thaw) {
            g.display_safety.resume_grace_until =
                Some(std::time::Instant::now() + RESUME_GRACE_WINDOW);
        }
        if g.display_safety.pending.is_some() || g.display_safety.manual_pending {
            g.display_safety.degraded =
                Some("Display operation crossed a lifecycle boundary".into());
            g.display_safety.manual_allowed = false;
        }
        let epoch = g.display_safety.lifecycle_epoch;
        if let Some(scope) = &mut g.session_agent.scope {
            scope.lifecycle_epoch = epoch;
        }
        g.recent_events.push(HardwareEvent::info(
            EventCategory::Service,
            format!("Lifecycle event: {phase:?}"),
            "rust-daemon",
        ));
        g.touch();
        if let Err(e) = g.save() {
            g.storage_fault = Some(e.clone());
            return Err(e);
        }
        g.session_agent.connected
    };
    if registered {
        if let Err(reason) = super::dock_transaction::bind_current_context(state).await {
            // Bind publishes policy only. A timeout here is not an unknown modeset.
            log::warn!("display scope bind deferred after lifecycle event: {reason}");
            let _ = logger::append_line(format!(
                "rust-daemon: display scope bind deferred after lifecycle event: {reason}"
            ));
        }
    }
    Ok(())
}
pub(crate) async fn handle_lifecycle_with<F>(
    state: &Arc<RwLock<RuntimeState>>,
    phase: LifecyclePhase,
    attached: bool,
    backlight: F,
) -> Result<(), String>
where
    F: FnOnce(u8) -> Result<(), String> + Send + 'static,
{
    let epoch_result = advance_display_epoch(state, phase.clone()).await;
    let (level, scale) = {
        let mut g = state.write().await;
        g.status.keyboard_attached = attached;
        (g.status.backlight_level, g.settings.default_scale)
    };
    match phase {
        LifecyclePhase::Boot => {
            mark_startup_replay_pending_from_boot().await;
        }
        LifecyclePhase::Post | LifecyclePhase::Thaw => {
            let _ = replay_current_dock_mode(state, attached, scale).await;
        }
        _ => {}
    }
    let level = if matches!(
        phase,
        LifecyclePhase::Pre | LifecyclePhase::Hibernate | LifecyclePhase::Shutdown
    ) {
        0
    } else {
        level
    };
    let backlight_result = tokio::task::spawn_blocking(move || backlight(level))
        .await
        .map_err(|e| e.to_string())?;
    epoch_result?;
    backlight_result
}
