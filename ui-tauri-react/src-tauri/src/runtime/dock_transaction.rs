//! Daemon-side single-owner automatic Dock transaction and receipt reconciliation.
use super::{display_safety::*, state::RuntimeState};
use crate::ipc::protocol::{SessionCommand, SessionResponse};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReconcileMode {
    Active,
    LateCleanup,
}
const QUERY_BUDGET: Duration = Duration::from_millis(1200);

fn save(state: &mut RuntimeState) -> Result<(), String> {
    state.touch();
    if let Err(reason) = state.save() {
        state.storage_fault = Some(reason.clone());
        state.display_safety.degraded = Some(reason.clone());
        state.display_safety.manual_allowed = false;
        return Err(reason);
    }
    Ok(())
}
fn scope_of(state: &RuntimeState) -> Result<DisplayScope, String> {
    if let Some(reason) = &state.storage_fault {
        return Err(reason.clone());
    }
    if !state.session_agent.connected {
        return Err("No session agent registered".into());
    }
    let scope = state
        .session_agent
        .scope
        .clone()
        .ok_or("No verified display scope")?;
    if scope.boot_id != state.display_safety.boot_id
        || scope.daemon_id != state.display_safety.daemon_id
        || scope.lifecycle_epoch != state.display_safety.lifecycle_epoch
    {
        return Err("Display scope is no longer current".into());
    }
    Ok(scope)
}
async fn rpc(
    state: &Arc<RwLock<RuntimeState>>,
    command: SessionCommand,
    budget: Duration,
) -> Result<SessionResponse, String> {
    timeout(
        budget,
        super::daemon::request_session(state.clone(), command, false),
    )
    .await
    .map_err(|_| "Display request budget exhausted".to_string())?
}
pub(crate) async fn bind_current_context(
    state: &Arc<RwLock<RuntimeState>>,
) -> Result<DisplayScope, String> {
    let (scope, automatic_blocked, manual_allowed) = {
        let g = state.read().await;
        (
            scope_of(&g)?,
            g.display_safety.degraded.is_some() || g.display_safety.suspended,
            g.display_safety.manual_allowed
                && g.display_safety.pending.is_none()
                && !g.display_safety.manual_pending
                && !g.display_safety.suspended,
        )
    };
    match rpc(
        state,
        SessionCommand::BindDisplayScope {
            scope: scope.clone(),
            automatic_blocked,
            manual_allowed,
        },
        QUERY_BUDGET,
    )
    .await?
    {
        SessionResponse::Ack => Ok(scope),
        SessionResponse::Error { message } => Err(message),
        _ => Err("Display context was not acknowledged".into()),
    }
}
async fn snapshot(state: &Arc<RwLock<RuntimeState>>) -> Result<DisplayExecutionSnapshot, String> {
    match rpc(
        state,
        SessionCommand::GetDisplayExecutionState,
        QUERY_BUDGET,
    )
    .await?
    {
        SessionResponse::DisplayExecutionState { snapshot } => Ok(snapshot),
        SessionResponse::Error { message } => Err(message),
        _ => Err("Unexpected execution-state reply".into()),
    }
}
async fn push_degraded_policy(state: &Arc<RwLock<RuntimeState>>) -> Result<(), String> {
    let (scope, manual_allowed) = {
        let g = state.read().await;
        (
            g.session_agent
                .scope
                .clone()
                .ok_or("No registered Agent for degraded policy")?,
            g.display_safety.manual_allowed,
        )
    };
    match rpc(
        state,
        SessionCommand::BindDisplayScope {
            scope,
            automatic_blocked: true,
            manual_allowed,
        },
        QUERY_BUDGET,
    )
    .await?
    {
        SessionResponse::Ack => Ok(()),
        _ => Err("Agent did not acknowledge degraded display policy".into()),
    }
}
pub(crate) async fn degrade(state: &Arc<RwLock<RuntimeState>>, reason: impl Into<String>) {
    {
        let mut g = state.write().await;
        g.display_safety.degraded = Some(reason.into());
        g.display_safety.manual_allowed = false;
        let _ = save(&mut g);
    }
    let _ = push_degraded_policy(state).await;
    super::daemon::notify_runtime_error(state,"Zenbook Duo Display Protection","Display result is unknown. Automatic display changes are paused for this boot; see the runtime journals before recovery.").await;
}
pub async fn submit_dock_target(
    state: &Arc<RwLock<RuntimeState>>,
    attached: bool,
    scale: f64,
    source: DockSource,
) -> Result<u64, String> {
    publish_dock_target(state, Some((attached, scale)), source).await
}
/// Replay samples the authoritative monitor state under the publication lock.
pub(crate) async fn submit_current_dock_target(
    state: &Arc<RwLock<RuntimeState>>,
    source: DockSource,
) -> Result<u64, String> {
    publish_dock_target(state, None, source).await
}
async fn publish_dock_target(
    state: &Arc<RwLock<RuntimeState>>,
    observed: Option<(bool, f64)>,
    source: DockSource,
) -> Result<u64, String> {
    let mut g = state.write().await;
    let (attached, scale) =
        observed.unwrap_or((g.status.keyboard_attached, g.settings.default_scale));
    if !scale.is_finite() || scale <= 0. {
        return Err("Invalid display scale".into());
    }
    if let Some(reason) = &g.storage_fault {
        return Err(reason.clone());
    }
    g.display_safety.next_target_generation = g
        .display_safety
        .next_target_generation
        .checked_add(1)
        .ok_or("Dock target generation exhausted")?;
    let generation = g.display_safety.next_target_generation;
    g.status.keyboard_attached = attached;
    g.display_safety.target = Some(DockTarget {
        attached,
        scale,
        generation,
        source,
    });
    save(&mut g)?;
    start_dock_worker(state, &mut g);
    Ok(generation)
}
fn start_dock_worker(state: &Arc<RwLock<RuntimeState>>, g: &mut RuntimeState) {
    if !g.display_safety.worker_running
        && g.display_safety.target.is_some()
        && g.display_safety.pending.is_none()
        && g.display_safety.receipt_to_consume.is_none()
        && !g.display_safety.manual_pending
        && g.display_safety.degraded.is_none()
        && !g.display_safety.suspended
        && g.storage_fault.is_none()
        && g.session_agent.connected
    {
        g.display_safety.worker_running = true;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(reason) = drain_dock_targets(state).await {
                log::warn!("automatic Dock blocked: {reason}");
            }
        });
    }
}
/// One bounded wakeup per daemon cleanup tick, only for unsent healthy intent.
pub(crate) async fn resume_dock_target(state: &Arc<RwLock<RuntimeState>>) {
    let mut g = state.write().await;
    start_dock_worker(state, &mut g);
}
pub async fn drain_dock_targets(state: Arc<RwLock<RuntimeState>>) -> Result<(), String> {
    let executor = state.read().await.display_executor.clone();
    let _execution = executor.lock().await;
    loop {
        let target = state.read().await.display_safety.target.clone();
        let Some(target) = target else {
            state.write().await.display_safety.worker_running = false;
            return Ok(());
        };
        let result = drain_one(&state, &target).await;
        let mut g = state.write().await;
        if result.is_err() {
            let changed = g
                .display_safety
                .target
                .as_ref()
                .is_some_and(|t| t.generation != target.generation);
            if changed
                && g.display_safety.pending.is_none()
                && g.display_safety.degraded.is_none()
                && g.storage_fault.is_none()
            {
                continue;
            }
            g.display_safety.worker_running = false;
            return result;
        }
        if g.display_safety.degraded.is_some()
            || g.storage_fault.is_some()
            || g.display_safety.pending.is_some()
        {
            g.display_safety.worker_running = false;
            return Err("Display operation requires reconciliation".into());
        }
        if g.display_safety.target.is_none() {
            g.display_safety.worker_running = false;
            return Ok(());
        }
    }
}
async fn drain_one(state: &Arc<RwLock<RuntimeState>>, target: &DockTarget) -> Result<(), String> {
    if state.read().await.display_safety.pending.is_some() {
        reconcile_pending_dock(state, ReconcileMode::Active).await?;
        return Ok(());
    }
    {
        let g = state.read().await;
        if let Some(reason) = &g.display_safety.degraded {
            return Err(reason.clone());
        }
        if g.display_safety.suspended {
            return Err("Display session is suspending".into());
        }
    }
    let scope = bind_current_context(state).await?;
    let snap = snapshot(state).await?;
    if snap.agent_id != scope.agent_id || snap.scope.as_ref() != Some(&scope) {
        return Err("Execution state is from another Agent".into());
    }
    if snap.blocked.is_some() {
        degrade(state, "Agent reports an unknown display outcome").await;
        return Err("Agent display result unknown".into());
    }
    if snap.active.is_some() || snap.receipt.is_some() {
        return Err("Agent display execution is busy".into());
    }
    match rpc(state, SessionCommand::GetDisplayLayout, QUERY_BUDGET).await? {
        SessionResponse::DisplayLayout { layout }
            if layout
                .displays
                .iter()
                .any(|d| d.connector == "eDP-1" && d.enabled) => {}
        _ => return Err("Display backend is not ready".into()),
    }
    let operation = {
        let mut g = state.write().await;
        if scope_of(&g)? != scope
            || g.display_safety.target.as_ref().map(|t| t.generation) != Some(target.generation)
        {
            return Ok(());
        }
        if g.display_safety
            .last_seen_revision
            .is_some_and(|r| r != snap.revision)
        {
            g.record_successful_dock_mode(false, false);
        }
        g.display_safety.last_seen_revision = Some(snap.revision);
        let panel = crate::hardware::sysfs::observe_secondary_panel(&g.drm_root);
        if !automatic_dock_allowed(
            target.attached,
            &panel,
            g.secondary_panel_disabled_by_runtime,
            false,
        ) {
            g.display_safety.target = None;
            save(&mut g)?;
            log::warn!(
                "display protection source={:?} boot={} target={} panel={panel:?} ownership={}",
                target.source,
                scope.boot_id,
                target.generation,
                g.secondary_panel_disabled_by_runtime
            );
            return Err("Automatic display protection: panel state or ownership is unknown".into());
        }
        g.display_safety.next_operation_id = g
            .display_safety
            .next_operation_id
            .checked_add(1)
            .ok_or("Display operation IDs exhausted")?;
        let op = DockOperation {
            scope: scope.clone(),
            operation_id: g.display_safety.next_operation_id,
            target_generation: target.generation,
            attached: target.attached,
            scale: target.scale,
            ownership_authorized: g.secondary_panel_disabled_by_runtime,
            expected_revision: snap.revision,
        };
        g.display_safety.pending = Some(op.clone());
        save(&mut g)?;
        op
    };
    log::info!(
        "display phase=dispatch scope={:?} operation={} target={} source={:?}",
        operation.scope,
        operation.operation_id,
        operation.target_generation,
        target.source
    );
    match rpc(
        state,
        SessionCommand::ApplyDockOperation {
            operation: operation.clone(),
        },
        Duration::from_secs(12),
    )
    .await
    {
        Ok(SessionResponse::DockReceipt { receipt }) if receipt.operation == operation => {
            finish_receipt(state, receipt, ReconcileMode::Active).await
        }
        _ => reconcile_pending_dock(state, ReconcileMode::Active).await,
    }
}
async fn finish_receipt(
    state: &Arc<RwLock<RuntimeState>>,
    receipt: DockReceipt,
    mode: ReconcileMode,
) -> Result<(), String> {
    let mut verified = false;
    let mut completed_current = false;
    {
        let mut g = state.write().await;
        if g.display_safety.pending.as_ref() != Some(&receipt.operation) {
            return Err("Receipt does not match pending display operation".into());
        }
        let latest = g
            .display_safety
            .target
            .as_ref()
            .map(|t| t.generation)
            .unwrap_or(0);
        let decision = scope_of(&g)
            .map(|s| classify_dock_commit(&s, latest, &receipt))
            .unwrap_or(CommitDecision::RejectStaleScope);
        let report = &receipt.report;
        let revision_ok = report.revision_before == receipt.operation.expected_revision
            && report.revision_after == report.revision_before.saturating_add(1);
        let observed = crate::hardware::sysfs::observe_secondary_panel(&g.drm_root);
        let physical_ok =
            observed == report.after && !matches!(observed, PanelObservation::Unknown { .. });
        let started = !matches!(report.status, MutationStatus::NotStarted { .. });
        if matches!(
            decision,
            CommitDecision::EvidenceAndTargetComplete | CommitDecision::EvidenceOnly
        ) && revision_ok
            && physical_ok
        {
            verified = true;
            if mode == ReconcileMode::Active && g.display_safety.degraded.is_none() {
                g.record_successful_dock_mode(
                    receipt.operation.attached,
                    matches!(report.before, PanelObservation::Enabled),
                );
                if decision == CommitDecision::EvidenceAndTargetComplete {
                    completed_current = true;
                    g.display_safety.target = None;
                }
            } else {
                g.record_successful_dock_mode(false, false);
                g.display_safety.manual_allowed = true;
            }
            g.display_safety.last_seen_revision = Some(report.revision_after);
        } else if decision == CommitDecision::NotStarted {
            let transient = matches!(&report.status,
                MutationStatus::NotStarted { reason } if reason == "Busy" || reason == "RevisionChanged");
            if !transient
                && g.display_safety.target.as_ref().map(|t| t.generation)
                    == Some(receipt.operation.target_generation)
            {
                g.display_safety.target = None;
            }
        } else {
            g.display_safety.degraded =
                Some(format!("Display result cannot be adopted: {decision:?}"));
            g.display_safety.manual_allowed = false;
        }
        if started && !verified {
            g.display_safety
                .degraded
                .get_or_insert("Display operation outcome is unknown".into());
        }
        g.display_safety.receipt_to_consume = Some(receipt.clone());
        save(&mut g)?;
    }
    consume_saved_receipt(state).await?;
    let _ = bind_current_context(state).await;
    if completed_current {
        let still_current = {
            let g = state.read().await;
            g.display_safety.target.is_none()
                && scope_of(&g).is_ok_and(|scope| scope == receipt.operation.scope)
        };
        if still_current {
            let state = state.clone();
            let attached = receipt.operation.attached;
            tokio::spawn(async move {
                let _ = rpc(
                    &state,
                    SessionCommand::ShowNotification {
                        title: "Zenbook Duo Control".into(),
                        message: if attached {
                            "Keyboard attached: bottom screen disabled".into()
                        } else {
                            "Keyboard detached: bottom screen enabled".into()
                        },
                        urgent: false,
                    },
                    QUERY_BUDGET,
                )
                .await;
            });
        }
    }
    if mode == ReconcileMode::Active && state.read().await.display_safety.degraded.is_some() {
        Err("Automatic display changes remain degraded".into())
    } else {
        Ok(())
    }
}
async fn consume_saved_receipt(state: &Arc<RwLock<RuntimeState>>) -> Result<(), String> {
    let receipt = {
        let mut g = state.write().await;
        if let Some(reason) = &g.storage_fault {
            return Err(reason.clone());
        }
        let Some(receipt) = g.display_safety.receipt_to_consume.clone() else {
            return Ok(());
        };
        // Never confuse an in-memory staged obligation with durable evidence.
        // Re-establish durability before every consume, including cleanup retries.
        save(&mut g)?;
        receipt
    };
    if state.read().await.display_safety.degraded.is_some() {
        push_degraded_policy(state).await?;
    }
    let response = rpc(
        state,
        SessionCommand::ConsumeDockReceipt {
            scope: receipt.operation.scope.clone(),
            operation_id: receipt.operation.operation_id,
        },
        QUERY_BUDGET,
    )
    .await;
    let acknowledged = matches!(response, Ok(SessionResponse::Ack));
    let not_started = matches!(receipt.report.status, MutationStatus::NotStarted { .. });
    if !acknowledged {
        let snap = snapshot(state).await?;
        if !(not_started
            && snap.agent_id == receipt.operation.scope.agent_id
            && snap.active.is_none()
            && snap.receipt.is_none())
        {
            return Err("Display receipt consumption remains pending".into());
        }
    }
    let mut g = state.write().await;
    if g.display_safety.receipt_to_consume.as_ref() != Some(&receipt) {
        return Err("Display consumption obligation changed".into());
    }
    g.display_safety.pending = None;
    g.display_safety.receipt_to_consume = None;
    save(&mut g)
}
pub async fn reconcile_pending_dock(
    state: &Arc<RwLock<RuntimeState>>,
    mode: ReconcileMode,
) -> Result<(), String> {
    if state
        .read()
        .await
        .display_safety
        .receipt_to_consume
        .is_some()
    {
        return consume_saved_receipt(state).await;
    }
    let Some(op) = state.read().await.display_safety.pending.clone() else {
        return Ok(());
    };
    for attempt in 0..if mode == ReconcileMode::Active { 3 } else { 1 } {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if let Ok(snap) = snapshot(state).await {
            if snap.agent_id == op.scope.agent_id {
                if let Some(receipt) = snap.receipt {
                    if receipt.operation == op {
                        return finish_receipt(state, receipt, mode).await;
                    }
                }
            }
        }
    }
    degrade(state, "Display result reconciliation budget exhausted").await;
    Err("Display outcome remains pending; no automatic retry permitted".into())
}

pub(crate) async fn apply_manual(
    state: &Arc<RwLock<RuntimeState>>,
    mutation: crate::ipc::protocol::ManualDisplayMutation,
) -> Result<MutationReport, String> {
    apply_non_dock(state, mutation, None).await
}
pub(crate) async fn apply_sensor(
    state: &Arc<RwLock<RuntimeState>>,
    scope: DisplayScope,
    orientation: crate::models::Orientation,
) -> Result<MutationReport, String> {
    let scale = state.read().await.settings.default_scale;
    apply_non_dock(
        state,
        crate::ipc::protocol::ManualDisplayMutation::Orientation { orientation, scale },
        Some(scope),
    )
    .await
}
async fn apply_non_dock(
    state: &Arc<RwLock<RuntimeState>>,
    mutation: crate::ipc::protocol::ManualDisplayMutation,
    sensor_scope: Option<DisplayScope>,
) -> Result<MutationReport, String> {
    let sensor = sensor_scope.is_some();
    let executor = state.read().await.display_executor.clone();
    let _execution = executor.lock().await;
    {
        let g = state.read().await;
        if let Some(expected) = &sensor_scope {
            if !g.settings.auto_rotate
                || g.display_safety.degraded.is_some()
                || scope_of(&g)? != *expected
            {
                return Err("Automatic rotation is disabled or its session changed".into());
            }
        }
        if g.display_safety.suspended
            || g.display_safety.pending.is_some()
            || g.display_safety.manual_pending
            || !g.display_safety.manual_allowed
        {
            return Err("Display outcome is pending or unknown".into());
        }
    }
    let scope = bind_current_context(state).await?;
    let snap = snapshot(state).await?;
    if snap.scope.as_ref() != Some(&scope)
        || snap.active.is_some()
        || snap.receipt.is_some()
        || snap.blocked.is_some()
    {
        return Err("Display execution is busy or cannot be verified".into());
    }
    {
        let mut g = state.write().await;
        if scope_of(&g)? != scope
            || sensor_scope.as_ref().is_some_and(|s| s != &scope)
            || (sensor && (!g.settings.auto_rotate || g.display_safety.degraded.is_some()))
        {
            return Err("Display session or automatic policy changed".into());
        }
        g.display_safety.manual_pending = true;
        save(&mut g)?;
    }
    let response = rpc(
        state,
        SessionCommand::ApplyDisplayMutation {
            sensor,
            scope: scope.clone(),
            mutation: mutation.clone(),
        },
        Duration::from_secs(12),
    )
    .await;
    match response {
        Ok(SessionResponse::MutationComplete { report }) => {
            let mut g = state.write().await;
            let current = scope_of(&g).is_ok_and(|s| s == scope);
            if current
                && matches!(
                    report.status,
                    MutationStatus::Verified | MutationStatus::NotStarted { .. }
                )
            {
                g.display_safety.manual_pending = false;
                if matches!(report.status, MutationStatus::Verified) {
                    if let crate::ipc::protocol::ManualDisplayMutation::Orientation {
                        orientation,
                        ..
                    } = &mutation
                    {
                        g.status.orientation = orientation.clone();
                    }
                    let keep_ownership = matches!(
                        mutation,
                        crate::ipc::protocol::ManualDisplayMutation::Orientation { .. }
                    ) && report.before == report.after
                        && g.display_safety.last_seen_revision == Some(report.revision_before);
                    if !keep_ownership {
                        g.record_successful_dock_mode(false, false);
                    }
                    g.display_safety.last_seen_revision = Some(report.revision_after);
                }
                save(&mut g)?;
                return if matches!(report.status, MutationStatus::Verified) {
                    Ok(report)
                } else {
                    Err("Display change did not start".into())
                };
            }
            drop(g);
            degrade(state, "Display result could not be verified").await;
            Err("Display operation is degraded".into())
        }
        _ => {
            degrade(state, "Display response is pending or unknown").await;
            Err("Display result is unknown; automatic changes paused".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::*;
    use crate::models::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use tokio::time::{timeout, Duration};
    struct Fixture {
        state: Arc<RwLock<RuntimeState>>,
        gate: Arc<AgentDisplayGate>,
        calls: Arc<AtomicUsize>,
        root: PathBuf,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn layout(secondary: bool) -> DisplayLayout {
        DisplayLayout {
            displays: [("eDP-1", true), ("eDP-2", secondary)]
                .into_iter()
                .map(|(c, enabled)| DisplayInfo {
                    connector: c.into(),
                    width: 2880,
                    height: 1800,
                    refresh_rate: 60.,
                    scale: 1.67,
                    x: 0,
                    y: 0,
                    transform: 0,
                    primary: c == "eDP-1",
                    enabled,
                    current_mode: DisplayMode {
                        mode_id: "2880x1800@60".into(),
                        width: 2880,
                        height: 1800,
                        refresh_rate: 60.,
                    },
                    available_modes: vec![],
                    refresh_policy: RefreshPolicy::Fixed,
                    supports_dynamic_refresh: false,
                })
                .collect(),
        }
    }
    async fn fixture(enabled: &str, delay: Option<Arc<tokio::sync::Notify>>) -> Fixture {
        let root =
            std::env::temp_dir().join(format!("duo-transaction-{}", new_identity().unwrap()));
        std::fs::create_dir_all(root.join("drm/card0-eDP-2")).unwrap();
        std::fs::write(root.join("drm/card0-eDP-2/enabled"), enabled).unwrap();
        let socket = root.join("agent.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let mut state = crate::runtime::test_utils::state();
        state.storage = Arc::new(super::super::store::RuntimeStore::new(
            root.join("state.json"),
        ));
        state.drm_root = root.join("drm");
        let scope = super::super::display_safety::tests::scope();
        state.display_safety.boot_id = scope.boot_id.clone();
        state.display_safety.daemon_id = scope.daemon_id.clone();
        state.session_agent.connected = true;
        state.session_agent.socket_path = Some(socket.to_string_lossy().into());
        state.session_agent.scope = Some(scope.clone());
        let gate = Arc::new(AgentDisplayGate::new("agent".into()));
        gate.bind_scope(scope).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let (g, c, r) = (gate.clone(), calls.clone(), root.clone());
        let server = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (g, c, r, delay) = (g.clone(), c.clone(), r.clone(), delay.clone());
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let line = BufReader::new(reader)
                        .lines()
                        .next_line()
                        .await
                        .unwrap()
                        .unwrap();
                    let env: Envelope<SessionCommand> = serde_json::from_str(&line).unwrap();
                    env.validate().unwrap();
                    let response = match env.payload {
                        SessionCommand::BindDisplayScope {
                            scope,
                            automatic_blocked,
                            manual_allowed,
                        } => {
                            if r.join("hold-bind").exists() {
                                std::fs::write(r.join("bind-entered"), "").unwrap();
                                while r.join("hold-bind").exists() {
                                    tokio::time::sleep(Duration::from_millis(5)).await;
                                }
                            }
                            match g.bind_context(scope, automatic_blocked, manual_allowed) {
                                Ok(()) => SessionResponse::Ack,
                                Err(message) => SessionResponse::Error { message },
                            }
                        }
                        SessionCommand::ShowNotification { .. } => SessionResponse::Ack,
                        SessionCommand::GetDisplayExecutionState => {
                            SessionResponse::DisplayExecutionState {
                                snapshot: g.snapshot(),
                            }
                        }
                        SessionCommand::GetDisplayLayout => SessionResponse::DisplayLayout {
                            layout: layout(
                                std::fs::read_to_string(r.join("drm/card0-eDP-2/enabled"))
                                    .unwrap_or_default()
                                    == "enabled",
                            ),
                        },
                        SessionCommand::ApplyDockOperation { operation } => {
                            c.fetch_add(1, Ordering::SeqCst);
                            if r.join("race-revision").exists() {
                                std::fs::remove_file(r.join("race-revision")).unwrap();
                                g.start_write(
                                    DisplayMutationKind::SensorRotation,
                                    operation.scope.clone(),
                                    None,
                                    || async { super::super::display_safety::tests::report() },
                                )
                                .unwrap()
                                .await
                                .unwrap();
                            }
                            let op = operation.clone();
                            let before =
                                crate::hardware::sysfs::observe_secondary_panel(&r.join("drm"));
                            match g.start_write(
                                DisplayMutationKind::Dock,
                                operation.scope.clone(),
                                Some(operation.clone()),
                                move || async move {
                                    if let Some(n) = delay {
                                        n.notified().await;
                                    }
                                    std::fs::write(
                                        r.join("drm/card0-eDP-2/enabled"),
                                        if op.attached { "disabled" } else { "enabled" },
                                    )
                                    .unwrap();
                                    MutationReport {
                                        before,
                                        after: if op.attached {
                                            PanelObservation::Disabled
                                        } else {
                                            PanelObservation::Enabled
                                        },
                                        primary_enabled: Some(true),
                                        secondary_enabled: Some(!op.attached),
                                        revision_before: 0,
                                        revision_after: 0,
                                        status: MutationStatus::Verified,
                                    }
                                },
                            ) {
                                Ok(h) => SessionResponse::DockReceipt {
                                    receipt: DockReceipt {
                                        operation,
                                        report: h.await.unwrap(),
                                    },
                                },
                                Err(e) => SessionResponse::DockReceipt {
                                    receipt: DockReceipt {
                                        operation,
                                        report: MutationReport::not_started(format!("{e:?}")),
                                    },
                                },
                            }
                        }
                        SessionCommand::ApplyDisplayMutation {
                            scope,
                            mutation,
                            sensor,
                        } => {
                            let kind = if sensor {
                                DisplayMutationKind::SensorRotation
                            } else if matches!(mutation, ManualDisplayMutation::Layout { .. }) {
                                DisplayMutationKind::ManualLayout
                            } else {
                                DisplayMutationKind::ManualOrientation
                            };
                            let before =
                                crate::hardware::sysfs::observe_secondary_panel(&r.join("drm"));
                            match g.start_write(kind, scope, None, move || async move {
                                if sensor {
                                    std::fs::write(r.join("sensor-entered"), "").unwrap();
                                    while r.join("hold-sensor").exists() {
                                        tokio::time::sleep(Duration::from_millis(5)).await;
                                    }
                                    if r.join("unknown-sensor").exists() {
                                        let mut report =
                                            super::super::display_safety::tests::report();
                                        report.status = MutationStatus::Unknown {
                                            reason: "injected sensor uncertainty".into(),
                                        };
                                        return report;
                                    }
                                }
                                let enabled = match mutation {
                                    ManualDisplayMutation::Layout { layout } => layout
                                        .displays
                                        .iter()
                                        .any(|d| d.connector == "eDP-2" && d.enabled),
                                    _ => matches!(before, PanelObservation::Enabled),
                                };
                                std::fs::write(
                                    r.join("drm/card0-eDP-2/enabled"),
                                    if enabled { "enabled" } else { "disabled" },
                                )
                                .unwrap();
                                MutationReport {
                                    before,
                                    after: if enabled {
                                        PanelObservation::Enabled
                                    } else {
                                        PanelObservation::Disabled
                                    },
                                    primary_enabled: Some(true),
                                    secondary_enabled: Some(enabled),
                                    revision_before: 0,
                                    revision_after: 0,
                                    status: MutationStatus::Verified,
                                }
                            }) {
                                Ok(h) => SessionResponse::MutationComplete {
                                    report: h.await.unwrap(),
                                },
                                Err(e) => SessionResponse::MutationComplete {
                                    report: MutationReport::not_started(format!("{e:?}")),
                                },
                            }
                        }
                        SessionCommand::ConsumeDockReceipt {
                            scope,
                            operation_id,
                        } => match g.consume(&scope, operation_id) {
                            Ok(()) => SessionResponse::Ack,
                            Err(message) => SessionResponse::Error { message },
                        },
                        _ => panic!("unexpected legacy command"),
                    };
                    let _ = writer
                        .write_all(
                            format!(
                                "{}\n",
                                serde_json::to_string(&Envelope::new(response)).unwrap()
                            )
                            .as_bytes(),
                        )
                        .await;
                });
            }
        });
        Fixture {
            state: Arc::new(RwLock::new(state)),
            gate,
            calls,
            root,
            server,
        }
    }
    async fn idle(f: &Fixture) {
        timeout(Duration::from_secs(3), async {
            loop {
                if !f.state.read().await.display_safety.worker_running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn all_sources_block_unknown_or_unowned_disabled_panel() {
        for observed in ["disabled", "invalid"] {
            for source in [
                DockSource::Monitor,
                DockSource::Startup,
                DockSource::Registration,
                DockSource::Resume,
            ] {
                let f = fixture(observed, None).await;
                submit_dock_target(&f.state, false, 1.67, source)
                    .await
                    .unwrap();
                idle(&f).await;
                assert_eq!(f.calls.load(Ordering::SeqCst), 0);
                assert!(!f.state.read().await.secondary_panel_disabled_by_runtime);
            }
        }
    }
    #[tokio::test]
    async fn normal_attach_and_detach_commit_once_and_release_receipt() {
        let f = fixture("enabled", None).await;
        submit_dock_target(&f.state, true, 1.67, DockSource::Monitor)
            .await
            .unwrap();
        idle(&f).await;
        assert!(f.state.read().await.secondary_panel_disabled_by_runtime);
        assert!(f.gate.snapshot().receipt.is_none());
        submit_dock_target(&f.state, false, 1.67, DockSource::Monitor)
            .await
            .unwrap();
        idle(&f).await;
        assert!(!f.state.read().await.secondary_panel_disabled_by_runtime);
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn latest_target_updates_while_old_attach_is_running() {
        let n = Arc::new(tokio::sync::Notify::new());
        let f = fixture("enabled", Some(n.clone())).await;
        submit_dock_target(&f.state, true, 1.67, DockSource::Startup)
            .await
            .unwrap();
        timeout(Duration::from_secs(1), async {
            while f.calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        submit_dock_target(&f.state, false, 1.67, DockSource::Monitor)
            .await
            .unwrap();
        n.notify_one();
        timeout(Duration::from_secs(1), async {
            while f.calls.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        n.notify_one();
        idle(&f).await;
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
        assert!(!f.state.read().await.secondary_panel_disabled_by_runtime);
        assert!(f.state.read().await.display_safety.target.is_none());
    }
    #[tokio::test]
    async fn not_found_pending_is_never_reissued_and_latches_degraded() {
        let f = fixture("enabled", None).await;
        f.state.write().await.display_safety.pending =
            Some(super::super::display_safety::tests::operation());
        assert!(reconcile_pending_dock(&f.state, ReconcileMode::Active)
            .await
            .is_err());
        let g = f.state.read().await;
        assert!(g.display_safety.pending.is_some());
        assert!(g.display_safety.degraded.is_some());
        assert!(!g.display_safety.manual_allowed);
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    }
    async fn completed_pending(f: &Fixture) {
        let op = super::super::display_safety::tests::operation();
        f.state.write().await.display_safety.pending = Some(op.clone());
        f.gate
            .start_write(
                DisplayMutationKind::Dock,
                op.scope.clone(),
                Some(op),
                || async { super::super::display_safety::tests::report() },
            )
            .unwrap()
            .await
            .unwrap();
        std::fs::write(f.root.join("drm/card0-eDP-2/enabled"), "disabled").unwrap();
    }
    #[tokio::test]
    async fn late_verified_receipt_restores_manual_only_and_consumes() {
        let f = fixture("enabled", None).await;
        completed_pending(&f).await;
        {
            let mut g = f.state.write().await;
            g.display_safety.degraded = Some("active budget exhausted".into());
            g.display_safety.manual_allowed = false;
        }
        reconcile_pending_dock(&f.state, ReconcileMode::LateCleanup)
            .await
            .unwrap();
        let g = f.state.read().await;
        assert!(g.display_safety.degraded.is_some());
        assert!(g.display_safety.manual_allowed);
        assert!(g.display_safety.pending.is_none());
        assert!(!g.secondary_panel_disabled_by_runtime);
        assert!(f.gate.snapshot().receipt.is_none());
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn changed_scope_cannot_adopt_old_receipt() {
        let f = fixture("enabled", None).await;
        completed_pending(&f).await;
        f.state
            .write()
            .await
            .session_agent
            .scope
            .as_mut()
            .unwrap()
            .lifecycle_epoch = 1;
        let _ = reconcile_pending_dock(&f.state, ReconcileMode::Active).await;
        let g = f.state.read().await;
        assert!(!g.secondary_panel_disabled_by_runtime);
        assert!(g.display_safety.degraded.is_some());
        assert!(!g.display_safety.manual_allowed);
        assert!(f.gate.snapshot().automatic_blocked);
        assert!(!f.gate.snapshot().manual_allowed);
    }
    #[tokio::test]
    async fn persistence_failure_prevents_dispatch_and_consumption() {
        let f = fixture("enabled", None).await;
        f.state.write().await.storage_fault = Some("injected storage failure".into());
        assert!(
            submit_dock_target(&f.state, true, 1.67, DockSource::Startup)
                .await
                .is_err()
        );
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        let f = fixture("enabled", None).await;
        completed_pending(&f).await;
        f.state.write().await.storage_fault = Some("injected storage failure".into());
        assert!(reconcile_pending_dock(&f.state, ReconcileMode::Active)
            .await
            .is_err());
        assert!(reconcile_pending_dock(&f.state, ReconcileMode::LateCleanup)
            .await
            .is_err());
        assert!(f.gate.snapshot().receipt.is_some());
        assert!(f.state.read().await.display_safety.pending.is_some());
    }
    #[tokio::test]
    async fn sensor_inflight_has_durable_obligation_before_backend_runs() {
        let f = fixture("enabled", None).await;
        f.state.write().await.settings.auto_rotate = true;
        std::fs::write(f.root.join("hold-sensor"), "").unwrap();
        let state = f.state.clone();
        let task = tokio::spawn(async move {
            apply_sensor(
                &state,
                super::super::display_safety::tests::scope(),
                Orientation::Normal,
            )
            .await
        });
        timeout(Duration::from_secs(1), async {
            while !f.root.join("sensor-entered").exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let storage = f.state.read().await.storage.clone();
        let mut loaded = RuntimeState::load_from(storage).unwrap();
        assert!(
            loaded.display_safety.manual_pending,
            "sensor execution must have a durable pre-execution obligation"
        );
        // Model daemon death in flight: durable pending survives process replacement.
        task.abort();
        loaded
            .display_safety
            .begin_daemon("boot", "replacement-daemon".into());
        assert!(loaded.display_safety.degraded.is_some());
        assert!(!loaded.display_safety.manual_allowed);
        assert!(loaded.display_safety.manual_pending);
        std::fs::remove_file(f.root.join("hold-sensor")).unwrap();
        // Owned Agent work still completes after its daemon waiter is gone.
        timeout(Duration::from_secs(1), async {
            while f.gate.snapshot().active.is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn sensor_unknown_survives_agent_and_daemon_replacement() {
        let f = fixture("enabled", None).await;
        f.state.write().await.settings.auto_rotate = true;
        std::fs::write(f.root.join("unknown-sensor"), "").unwrap();
        assert!(apply_sensor(
            &f.state,
            super::super::display_safety::tests::scope(),
            Orientation::Normal
        )
        .await
        .is_err());
        let mut loaded = RuntimeState::load_from(f.state.read().await.storage.clone()).unwrap();
        loaded
            .display_safety
            .begin_daemon("boot", "replacement-daemon".into());
        assert!(loaded.display_safety.manual_pending);
        assert!(loaded.display_safety.degraded.is_some());
        let socket = f
            .state
            .read()
            .await
            .session_agent
            .socket_path
            .clone()
            .unwrap();
        super::super::daemon::handle_session_registration(
            &f.state,
            "desktop".into(),
            SessionBackend::Gnome,
            socket,
            "replacement-agent".into(),
        )
        .await
        .unwrap();
        let scope = f.state.read().await.session_agent.scope.clone().unwrap();
        let replacement = Arc::new(AgentDisplayGate::new("replacement-agent".into()));
        let safety = f.state.read().await.display_safety.clone();
        replacement
            .bind_context(
                scope.clone(),
                safety.degraded.is_some(),
                safety.manual_allowed && !safety.manual_pending,
            )
            .unwrap();
        assert!(replacement
            .start_write(
                DisplayMutationKind::SensorRotation,
                scope.clone(),
                None,
                || async { panic!("replacement must not write") }
            )
            .is_err());
        assert!(replacement
            .start_write(
                DisplayMutationKind::ManualOrientation,
                scope,
                None,
                || async { panic!("replacement must not write") }
            )
            .is_err());
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn sensor_respects_degraded_automatic_policy_even_when_manual_is_allowed() {
        let f = fixture("enabled", None).await;
        {
            let mut g = f.state.write().await;
            g.settings.auto_rotate = true;
            g.display_safety.degraded = Some("late verified Dock".into());
            g.display_safety.manual_allowed = true;
        }
        assert!(apply_sensor(
            &f.state,
            super::super::display_safety::tests::scope(),
            Orientation::Normal
        )
        .await
        .is_err());
        assert!(!f.root.join("sensor-entered").exists());
    }
    #[tokio::test]
    async fn busy_sensor_keeps_latest_target_and_cleanup_resumes_it() {
        let f = fixture("enabled", None).await;
        let n = Arc::new(tokio::sync::Notify::new());
        let release = n.clone();
        let task = f
            .gate
            .start_write(
                DisplayMutationKind::SensorRotation,
                super::super::display_safety::tests::scope(),
                None,
                move || async move {
                    release.notified().await;
                    super::super::display_safety::tests::report()
                },
            )
            .unwrap();
        submit_dock_target(&f.state, true, 1.67, DockSource::Monitor)
            .await
            .unwrap();
        idle(&f).await;
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        n.notify_one();
        task.await.unwrap();
        // Exercise exactly the daemon's periodic cleanup entry point.
        super::super::daemon::cleanup_display_work(&f.state).await;
        idle(&f).await;
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        assert!(f.state.read().await.display_safety.target.is_none());
    }
    #[tokio::test]
    async fn revision_changed_before_admission_reobserves_latest_target() {
        let f = fixture("enabled", None).await;
        std::fs::write(f.root.join("race-revision"), "").unwrap();
        submit_dock_target(&f.state, true, 1.67, DockSource::Monitor)
            .await
            .unwrap();
        idle(&f).await;
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
        assert!(f.state.read().await.display_safety.pending.is_none());
        assert!(f.state.read().await.display_safety.target.is_none());
        assert_eq!(
            std::fs::read_to_string(f.root.join("drm/card0-eDP-2/enabled")).unwrap(),
            "disabled"
        );
    }
    #[tokio::test]
    async fn registration_replay_does_not_overwrite_new_keyboard_observation() {
        let f = fixture("enabled", None).await;
        std::fs::write(f.root.join("hold-bind"), "").unwrap();
        let socket = f
            .state
            .read()
            .await
            .session_agent
            .socket_path
            .clone()
            .unwrap();
        super::super::daemon::handle_session_registration(
            &f.state,
            "desktop".into(),
            SessionBackend::Gnome,
            socket,
            "agent".into(),
        )
        .await
        .unwrap();
        timeout(Duration::from_secs(1), async {
            while !f.root.join("bind-entered").exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // New monitor observation is authoritative while registration is waiting.
        f.state.write().await.status.keyboard_attached = true;
        std::fs::remove_file(f.root.join("hold-bind")).unwrap();
        timeout(Duration::from_secs(2), async {
            while f.calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        idle(&f).await;
        assert!(f.state.read().await.status.keyboard_attached);
        assert_eq!(
            std::fs::read_to_string(f.root.join("drm/card0-eDP-2/enabled")).unwrap(),
            "disabled"
        );
    }
    #[tokio::test]
    async fn delayed_manual_reply_cannot_preserve_old_ownership() {
        let f = fixture("enabled", None).await;
        {
            let mut g = f.state.write().await;
            g.secondary_panel_disabled_by_runtime = true;
            g.display_safety.last_seen_revision = Some(0);
        }
        f.gate
            .start_write(
                DisplayMutationKind::ManualLayout,
                super::super::display_safety::tests::scope(),
                None,
                || async { super::super::display_safety::tests::report() },
            )
            .unwrap()
            .await
            .unwrap();
        std::fs::write(f.root.join("drm/card0-eDP-2/enabled"), "disabled").unwrap();
        submit_dock_target(&f.state, false, 1.67, DockSource::Monitor)
            .await
            .unwrap();
        idle(&f).await;
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        assert!(!f.state.read().await.secondary_panel_disabled_by_runtime);
    }
    #[tokio::test]
    async fn actual_response_timeout_does_not_release_a_second_mutation() {
        let n = Arc::new(tokio::sync::Notify::new());
        let f = fixture("enabled", Some(n.clone())).await;
        submit_dock_target(&f.state, true, 1.67, DockSource::Resume)
            .await
            .unwrap();
        timeout(Duration::from_secs(16), async {
            while f.state.read().await.display_safety.worker_running {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        assert!(f.gate.snapshot().active.is_some());
        assert!(f.state.read().await.display_safety.degraded.is_some());
        submit_dock_target(&f.state, false, 1.67, DockSource::Monitor)
            .await
            .unwrap();
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        n.notify_one();
        timeout(Duration::from_secs(1), async {
            while f.gate.snapshot().receipt.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        reconcile_pending_dock(&f.state, ReconcileMode::LateCleanup)
            .await
            .unwrap();
        assert!(f.state.read().await.display_safety.manual_allowed);
        assert!(f.state.read().await.display_safety.degraded.is_some());
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn real_startup_respects_executor_and_registration_ack_is_nonblocking() {
        let f = fixture("enabled", None).await;
        let executor = f.state.read().await.display_executor.clone();
        let held = executor.lock().await;
        f.state.write().await.status.keyboard_attached = true;
        super::super::daemon::run_startup_replay_once(f.state.clone()).await;
        tokio::task::yield_now().await;
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        drop(held);
        idle(&f).await;
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        let n = Arc::new(tokio::sync::Notify::new());
        let f = fixture("enabled", Some(n.clone())).await;
        let socket = f
            .state
            .read()
            .await
            .session_agent
            .socket_path
            .clone()
            .unwrap();
        timeout(
            Duration::from_millis(300),
            super::super::daemon::handle_session_registration(
                &f.state,
                "desktop".into(),
                SessionBackend::Gnome,
                socket,
                "agent".into(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        timeout(Duration::from_secs(1), async {
            while f.calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(f.gate.snapshot().active.is_some());
        n.notify_one();
        idle(&f).await;
    }
    #[tokio::test]
    async fn manual_orientation_updates_status_without_deadlock_and_layout_revokes_ownership() {
        let f = fixture("disabled", None).await;
        {
            let mut g = f.state.write().await;
            g.secondary_panel_disabled_by_runtime = true;
            g.display_safety.last_seen_revision = Some(0);
        }
        let response = timeout(
            Duration::from_secs(1),
            super::super::daemon::apply_orientation(&f.state, Orientation::Left),
        )
        .await
        .unwrap();
        assert!(matches!(response, DaemonResponse::Ack));
        assert_eq!(f.state.read().await.status.orientation, Orientation::Left);
        assert!(!f.state.read().await.display_safety.manual_pending);
        apply_manual(
            &f.state,
            ManualDisplayMutation::Layout {
                layout: layout(false),
            },
        )
        .await
        .unwrap();
        assert!(!f.state.read().await.secondary_panel_disabled_by_runtime);
    }
    #[tokio::test]
    async fn safety_lifecycle_publishes_target_even_when_backlight_fails() {
        let f = fixture("enabled", None).await;
        let outcome = super::super::daemon::handle_lifecycle_with(
            &f.state,
            LifecyclePhase::Post,
            true,
            |_| Err("fake HID failure".into()),
        )
        .await;
        assert!(outcome.is_err());
        idle(&f).await;
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        assert!(f.state.read().await.secondary_panel_disabled_by_runtime);
    }
    #[tokio::test]
    async fn safety_lifecycle_invalidates_inflight_but_preserves_previously_committed_ownership() {
        let n = Arc::new(tokio::sync::Notify::new());
        let f = fixture("enabled", Some(n.clone())).await;
        submit_dock_target(&f.state, true, 1.67, DockSource::Monitor)
            .await
            .unwrap();
        timeout(Duration::from_secs(1), async {
            while f.gate.snapshot().active.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        super::super::daemon::advance_display_epoch(&f.state, LifecyclePhase::Pre)
            .await
            .unwrap();
        super::super::daemon::advance_display_epoch(&f.state, LifecyclePhase::Post)
            .await
            .unwrap();
        n.notify_one();
        idle(&f).await;
        assert!(!f.state.read().await.secondary_panel_disabled_by_runtime);
        assert!(f.state.read().await.display_safety.degraded.is_some());
        assert!(f.gate.snapshot().automatic_blocked);
        let f = fixture("disabled", None).await;
        f.state.write().await.secondary_panel_disabled_by_runtime = true;
        super::super::daemon::advance_display_epoch(&f.state, LifecyclePhase::Pre)
            .await
            .unwrap();
        assert!(f.state.read().await.secondary_panel_disabled_by_runtime);
    }
    #[tokio::test]
    async fn safety_fast_keyboard_publication_cannot_be_undone_by_slow_probe() {
        let f = fixture("enabled", None).await;
        let mut old = f.state.read().await.status.clone();
        old.keyboard_attached = false;
        let side = super::super::monitor::publish_keyboard_state(&f.state, true)
            .await
            .unwrap();
        assert!(side
            .iter()
            .any(|a| matches!(a, super::super::policy::PolicyAction::SetBacklight(_))));
        // The caller has not run its slow HID/radio side work yet.
        assert!(f.state.read().await.display_safety.target.is_some());
        super::super::monitor::apply_polled_status(&f.state, old)
            .await
            .unwrap();
        assert!(f.state.read().await.status.keyboard_attached);
        idle(&f).await;
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn safety_crash_after_receipt_consumption_can_finish_durable_obligation() {
        let f = fixture("enabled", None).await;
        completed_pending(&f).await;
        let receipt = f.gate.snapshot().receipt.unwrap();
        {
            let mut g = f.state.write().await;
            g.display_safety.receipt_to_consume = Some(receipt.clone());
            g.touch();
            g.save().unwrap();
        }
        f.gate
            .consume(&receipt.operation.scope, receipt.operation.operation_id)
            .unwrap();
        reconcile_pending_dock(&f.state, ReconcileMode::Active)
            .await
            .unwrap();
        let g = f.state.read().await;
        assert!(g.display_safety.pending.is_none());
        assert!(g.display_safety.receipt_to_consume.is_none());
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    }
}
