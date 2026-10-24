use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PanelObservation {
    Enabled,
    Disabled,
    Unknown { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayScope {
    pub boot_id: String,
    pub daemon_id: String,
    pub agent_id: String,
    pub registration_generation: u64,
    pub lifecycle_epoch: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockOperation {
    pub scope: DisplayScope,
    pub operation_id: u64,
    pub target_generation: u64,
    pub attached: bool,
    pub scale: f64,
    pub ownership_authorized: bool,
    pub expected_revision: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayMutationKind {
    Dock,
    ManualLayout,
    ManualOrientation,
    SensorRotation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MutationStatus {
    Verified,
    NotStarted { reason: String },
    KnownFailure { reason: String },
    Unknown { reason: String },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationReport {
    pub before: PanelObservation,
    pub after: PanelObservation,
    pub primary_enabled: Option<bool>,
    pub secondary_enabled: Option<bool>,
    pub revision_before: u64,
    pub revision_after: u64,
    pub status: MutationStatus,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockReceipt {
    pub operation: DockOperation,
    pub report: MutationReport,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationRecord {
    pub kind: DisplayMutationKind,
    pub report: MutationReport,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveMutation {
    pub kind: DisplayMutationKind,
    pub operation: Option<DockOperation>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayExecutionSnapshot {
    pub agent_id: String,
    pub scope: Option<DisplayScope>,
    pub revision: u64,
    pub active: Option<ActiveMutation>,
    pub receipt: Option<DockReceipt>,
    pub last_mutation: Option<MutationRecord>,
    pub blocked: Option<String>,
    pub automatic_blocked: bool,
    pub manual_allowed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DockSource {
    Monitor,
    Startup,
    Registration,
    Resume,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockTarget {
    pub attached: bool,
    pub scale: f64,
    pub generation: u64,
    pub source: DockSource,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySafetyState {
    pub boot_id: String,
    pub daemon_id: String,
    pub lifecycle_epoch: u64,
    pub suspended: bool,
    pub pending: Option<DockOperation>,
    pub manual_pending: bool,
    pub receipt_to_consume: Option<DockReceipt>,
    pub degraded: Option<String>,
    pub manual_allowed: bool,
    pub target: Option<DockTarget>,
    pub next_operation_id: u64,
    pub next_target_generation: u64,
    pub last_seen_revision: Option<u64>,
    #[serde(skip)]
    pub worker_running: bool,
    #[serde(skip)]
    pub resume_grace_until: Option<std::time::Instant>,
    #[serde(skip)]
    pub closed_lid_retry_not_before: Option<std::time::Instant>,
}
impl Default for DisplaySafetyState {
    fn default() -> Self {
        Self {
            boot_id: String::new(),
            daemon_id: String::new(),
            lifecycle_epoch: 0,
            suspended: false,
            pending: None,
            manual_pending: false,
            receipt_to_consume: None,
            degraded: None,
            manual_allowed: true,
            target: None,
            next_operation_id: 0,
            next_target_generation: 0,
            last_seen_revision: None,
            worker_running: false,
            resume_grace_until: None,
            closed_lid_retry_not_before: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayReject {
    Busy,
    StaleScope,
    RevisionChanged,
    InvalidOperation,
    Degraded,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitDecision {
    EvidenceAndTargetComplete,
    EvidenceOnly,
    NotStarted,
    RejectStaleScope,
    Unknown,
}

pub fn automatic_dock_allowed(
    _attached: bool,
    observed: &PanelObservation,
    ownership: bool,
    uncertain: bool,
) -> bool {
    !uncertain
        && match observed {
            PanelObservation::Enabled => true,
            PanelObservation::Disabled => _attached || ownership,
            PanelObservation::Unknown { .. } => false,
        }
}
pub(crate) fn closed_laptop_panel_refusal(reason: &str) -> bool {
    reason.contains("Refusing to activate a closed laptop panel")
}

pub fn classify_dock_commit(
    current_scope: &DisplayScope,
    latest_target: u64,
    receipt: &DockReceipt,
) -> CommitDecision {
    if &receipt.operation.scope != current_scope {
        return CommitDecision::RejectStaleScope;
    }
    match &receipt.report.status {
        MutationStatus::NotStarted { .. } => CommitDecision::NotStarted,
        MutationStatus::KnownFailure { reason } if closed_laptop_panel_refusal(reason) => {
            CommitDecision::NotStarted
        }
        MutationStatus::Verified
            if receipt.report.primary_enabled == Some(true)
                && receipt.report.secondary_enabled == Some(!receipt.operation.attached)
                && receipt.report.after
                    == if receipt.operation.attached {
                        PanelObservation::Disabled
                    } else {
                        PanelObservation::Enabled
                    } =>
        {
            if latest_target == receipt.operation.target_generation {
                CommitDecision::EvidenceAndTargetComplete
            } else {
                CommitDecision::EvidenceOnly
            }
        }
        _ => CommitDecision::Unknown,
    }
}

pub fn new_identity() -> Result<String, String> {
    let id = std::fs::read_to_string("/proc/sys/kernel/random/uuid")
        .map_err(|e| format!("Cannot allocate runtime identity: {e}"))?;
    let id = id.trim();
    if id.len() != 36 {
        return Err("Invalid runtime identity".into());
    }
    Ok(id.into())
}
impl DisplaySafetyState {
    pub fn begin_daemon(&mut self, boot: &str, daemon_id: String) {
        if self.boot_id != boot {
            *self = Self::default();
        } else if self.pending.is_some() || self.receipt_to_consume.is_some() || self.manual_pending
        {
            self.degraded = Some("Previous daemon left a display operation pending".into());
            self.manual_allowed = false;
        }
        self.boot_id = boot.into();
        self.daemon_id = daemon_id;
        self.worker_running = false;
        self.target = None;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn scope() -> DisplayScope {
        DisplayScope {
            boot_id: "boot".into(),
            daemon_id: "daemon".into(),
            agent_id: "agent".into(),
            registration_generation: 1,
            lifecycle_epoch: 0,
        }
    }
    pub fn operation() -> DockOperation {
        DockOperation {
            scope: scope(),
            operation_id: 1,
            target_generation: 1,
            attached: true,
            scale: 1.67,
            ownership_authorized: false,
            expected_revision: 0,
        }
    }
    pub fn report() -> MutationReport {
        MutationReport {
            before: PanelObservation::Enabled,
            after: PanelObservation::Disabled,
            primary_enabled: Some(true),
            secondary_enabled: Some(false),
            revision_before: 0,
            revision_after: 1,
            status: MutationStatus::Verified,
        }
    }
    #[test]
    fn automatic_open_requires_known_state_and_ownership() {
        for (observed, owned, uncertain, want) in [
            (PanelObservation::Disabled, false, false, false),
            (PanelObservation::Disabled, true, false, true),
            (PanelObservation::Enabled, false, false, true),
            (
                PanelObservation::Unknown {
                    reason: "read failed".into(),
                },
                true,
                false,
                false,
            ),
            (PanelObservation::Enabled, true, true, false),
        ] {
            assert_eq!(
                automatic_dock_allowed(false, &observed, owned, uncertain),
                want
            );
        }
    }
    #[test]
    fn old_target_can_supply_effect_but_old_scope_cannot() {
        let r = DockReceipt {
            operation: operation(),
            report: report(),
        };
        assert_eq!(
            classify_dock_commit(&scope(), 2, &r),
            CommitDecision::EvidenceOnly
        );
        assert_eq!(
            classify_dock_commit(&scope(), 1, &r),
            CommitDecision::EvidenceAndTargetComplete
        );
        for field in 0..5 {
            let mut other = scope();
            match field {
                0 => other.boot_id = "other".into(),
                1 => other.daemon_id = "other".into(),
                2 => other.agent_id = "other".into(),
                3 => other.registration_generation += 1,
                _ => other.lifecycle_epoch += 1,
            };
            assert_eq!(
                classify_dock_commit(&other, 1, &r),
                CommitDecision::RejectStaleScope
            );
        }
    }
    #[test]
    fn unverifiable_report_cannot_grant_ownership() {
        let mut r = DockReceipt {
            operation: operation(),
            report: report(),
        };
        r.report.status = MutationStatus::Unknown {
            reason: "partial".into(),
        };
        assert_eq!(
            classify_dock_commit(&scope(), 1, &r),
            CommitDecision::Unknown
        );
        r.report = report();
        r.report.after = PanelObservation::Enabled;
        assert_eq!(
            classify_dock_commit(&scope(), 1, &r),
            CommitDecision::Unknown
        );
        r.report.status = MutationStatus::Verified;
        r.report.secondary_enabled = Some(true);
        assert_eq!(
            classify_dock_commit(&scope(), 1, &r),
            CommitDecision::Unknown
        );
    }
    #[test]
    fn safety_daemon_restart_keeps_uncertainty_but_new_boot_discards_old_operations() {
        let mut s = DisplaySafetyState {
            boot_id: "boot".into(),
            daemon_id: "old".into(),
            pending: Some(operation()),
            manual_pending: true,
            ..DisplaySafetyState::default()
        };
        s.begin_daemon("boot", "new".into());
        assert!(s.degraded.is_some());
        assert!(!s.manual_allowed);
        assert!(s.pending.is_some());
        s.begin_daemon("other-boot", "newer".into());
        assert!(s.pending.is_none());
        assert!(!s.manual_pending);
        assert!(s.degraded.is_none());
        assert_eq!(s.boot_id, "other-boot");
    }
}

use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

pub struct AgentDisplayGate {
    inner: Mutex<DisplayExecutionSnapshot>,
    writer: Arc<AsyncMutex<()>>,
    consumed: Mutex<Option<(DisplayScope, u64)>>,
}
impl AgentDisplayGate {
    pub fn new(agent_id: String) -> Self {
        Self {
            inner: Mutex::new(DisplayExecutionSnapshot {
                agent_id,
                scope: None,
                revision: 0,
                active: None,
                receipt: None,
                last_mutation: None,
                blocked: None,
                automatic_blocked: false,
                manual_allowed: true,
            }),
            writer: Arc::new(AsyncMutex::new(())),
            consumed: Mutex::new(None),
        }
    }
    pub fn bind_scope(&self, scope: DisplayScope) -> Result<(), String> {
        self.bind_context_inner(scope, None)
    }
    pub fn bind_context(
        &self,
        scope: DisplayScope,
        automatic_blocked: bool,
        manual_allowed: bool,
    ) -> Result<(), String> {
        self.bind_context_inner(scope, Some((automatic_blocked, manual_allowed)))
    }
    fn bind_context_inner(
        &self,
        scope: DisplayScope,
        policy: Option<(bool, bool)>,
    ) -> Result<(), String> {
        let mut g = self.inner.lock().expect("display state poisoned");
        if scope.agent_id != g.agent_id || scope.boot_id.is_empty() || scope.daemon_id.is_empty() {
            return Err("Invalid display registration scope".into());
        }
        if let Some(old) = &g.scope {
            if old.daemon_id == scope.daemon_id
                && (scope.registration_generation < old.registration_generation
                    || scope.lifecycle_epoch < old.lifecycle_epoch)
            {
                return Err("Stale display context".into());
            }
        }
        if g.scope.as_ref() != Some(&scope) && g.active.is_some() {
            g.blocked = Some("Display scope changed during an operation".into());
        }
        g.scope = Some(scope);
        if let Some((automatic_blocked, manual_allowed)) = policy {
            g.automatic_blocked = automatic_blocked;
            g.manual_allowed = manual_allowed;
        }
        Ok(())
    }
    pub fn snapshot(&self) -> DisplayExecutionSnapshot {
        self.inner.lock().expect("display state poisoned").clone()
    }
    pub fn set_policy(&self, automatic_blocked: bool, manual_allowed: bool) {
        let mut g = self.inner.lock().expect("display state poisoned");
        g.automatic_blocked = automatic_blocked;
        g.manual_allowed = manual_allowed;
    }
    pub fn consume(&self, scope: &DisplayScope, operation_id: u64) -> Result<(), String> {
        let mut g = self.inner.lock().expect("display state poisoned");
        let mut consumed = self.consumed.lock().expect("display consumption poisoned");
        match &g.receipt {
            Some(r) if &r.operation.scope == scope && r.operation.operation_id == operation_id => {
                *consumed = Some((scope.clone(), operation_id));
                g.receipt = None;
                Ok(())
            }
            None if consumed.as_ref() == Some(&(scope.clone(), operation_id)) => Ok(()),
            _ => Err("No matching completed display receipt".into()),
        }
    }
    pub fn start_write<F, Fut>(
        self: &Arc<Self>,
        kind: DisplayMutationKind,
        scope: DisplayScope,
        operation: Option<DockOperation>,
        run: F,
    ) -> Result<tokio::task::JoinHandle<MutationReport>, DisplayReject>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = MutationReport> + Send + 'static,
    {
        let mut g = self.inner.lock().expect("display state poisoned");
        if g.scope.as_ref() != Some(&scope) {
            return Err(DisplayReject::StaleScope);
        }
        if let Some(op) = &operation {
            if !op.scale.is_finite()
                || op.scale <= 0.
                || op.scope != scope
                || kind != DisplayMutationKind::Dock
            {
                return Err(DisplayReject::InvalidOperation);
            }
            if let Some(r) = &g.receipt {
                if r.operation == *op {
                    let report = r.report.clone();
                    return Ok(tokio::spawn(async move { report }));
                }
                return Err(if r.operation.operation_id == op.operation_id {
                    DisplayReject::InvalidOperation
                } else {
                    DisplayReject::Busy
                });
            }
            if self
                .consumed
                .lock()
                .expect("display consumption poisoned")
                .as_ref()
                .is_some_and(|(s, id)| s == &scope && op.operation_id <= *id)
            {
                return Err(DisplayReject::InvalidOperation);
            }
            if op.expected_revision != g.revision {
                return Err(DisplayReject::RevisionChanged);
            }
        } else if kind == DisplayMutationKind::Dock {
            return Err(DisplayReject::InvalidOperation);
        }
        if g.blocked.is_some()
            || (!g.manual_allowed
                && matches!(
                    kind,
                    DisplayMutationKind::ManualLayout | DisplayMutationKind::ManualOrientation
                ))
            || (g.automatic_blocked
                && matches!(
                    kind,
                    DisplayMutationKind::Dock | DisplayMutationKind::SensorRotation
                ))
        {
            return Err(DisplayReject::Degraded);
        }
        if g.active.is_some() || g.receipt.is_some() {
            return Err(DisplayReject::Busy);
        }
        let writer = self
            .writer
            .clone()
            .try_lock_owned()
            .map_err(|_| DisplayReject::Busy)?;
        let revision = g.revision;
        g.active = Some(ActiveMutation {
            kind,
            operation: operation.clone(),
        });
        drop(g);
        let gate = self.clone();
        Ok(tokio::spawn(async move {
            use futures_util::FutureExt;
            let _writer = writer;
            let result = std::panic::AssertUnwindSafe(async move { run().await })
                .catch_unwind()
                .await;
            let mut report =
                result.unwrap_or_else(|_| MutationReport::unknown("Display worker panicked"));
            let mut g = gate.inner.lock().expect("display state poisoned");
            report.revision_before = revision;
            let started = !matches!(report.status, MutationStatus::NotStarted { .. });
            if started {
                g.revision = g.revision.saturating_add(1);
            }
            report.revision_after = g.revision;
            if matches!(report.status, MutationStatus::Unknown { .. }) {
                g.blocked = Some("Display result could not be verified".into());
            }
            if let MutationStatus::Unknown { reason } = &report.status {
                log::warn!("display mutation kind={kind:?} scope={scope:?} outcome=unknown reason={reason} before={:?} after={:?}",report.before,report.after);
            }
            log::info!(
                "display mutation kind={kind:?} scope={scope:?} outcome={:?} revision={}",
                report.status,
                report.revision_after
            );
            g.last_mutation = Some(MutationRecord {
                kind,
                report: report.clone(),
            });
            if let Some(operation) = operation {
                g.receipt = Some(DockReceipt {
                    operation,
                    report: report.clone(),
                });
            }
            g.active = None;
            report
        }))
    }
}
impl MutationReport {
    pub fn unknown(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            before: PanelObservation::Unknown {
                reason: reason.clone(),
            },
            after: PanelObservation::Unknown {
                reason: reason.clone(),
            },
            primary_enabled: None,
            secondary_enabled: None,
            revision_before: 0,
            revision_after: 0,
            status: MutationStatus::Unknown { reason },
        }
    }
    pub fn not_started(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        let mut r = Self::unknown(reason.clone());
        r.status = MutationStatus::NotStarted { reason };
        r
    }
}

#[cfg(test)]
mod agent_tests {
    use super::tests::{operation, report, scope};
    use super::*;
    #[tokio::test]
    async fn agent_gate_remains_owned_after_reply_waiter_is_dropped() {
        let gate = Arc::new(AgentDisplayGate::new("agent".into()));
        gate.bind_scope(scope()).unwrap();
        let (send, recv) = tokio::sync::oneshot::channel();
        let task = gate
            .start_write(
                DisplayMutationKind::Dock,
                scope(),
                Some(operation()),
                || async move {
                    recv.await.unwrap();
                    report()
                },
            )
            .unwrap();
        drop(task);
        for kind in [
            DisplayMutationKind::ManualLayout,
            DisplayMutationKind::ManualOrientation,
            DisplayMutationKind::SensorRotation,
        ] {
            assert!(matches!(
                gate.start_write(kind, scope(), None, || async {
                    panic!("Busy write executed")
                }),
                Err(DisplayReject::Busy)
            ));
        }
        assert!(gate.snapshot().active.is_some());
        send.send(()).unwrap();
        for _ in 0..10 {
            tokio::task::yield_now().await;
            if gate.snapshot().receipt.is_some() {
                break;
            }
        }
        assert!(gate.snapshot().active.is_none());
        assert_eq!(gate.snapshot().receipt.unwrap().operation.operation_id, 1);
        assert!(gate.consume(&scope(), 2).is_err());
        gate.consume(&scope(), 1).unwrap();
        gate.consume(&scope(), 1).unwrap();
        assert!(gate
            .start_write(
                DisplayMutationKind::Dock,
                scope(),
                Some(operation()),
                || async { panic!("consumed operation replayed") }
            )
            .is_err());
    }
    #[tokio::test]
    async fn agent_rejects_stale_revision_and_unknown_result_stays_blocked() {
        let gate = Arc::new(AgentDisplayGate::new("agent".into()));
        gate.bind_scope(scope()).unwrap();
        let task = gate
            .start_write(
                DisplayMutationKind::ManualOrientation,
                scope(),
                None,
                || async { report() },
            )
            .unwrap();
        task.await.unwrap();
        assert!(matches!(
            gate.start_write(
                DisplayMutationKind::Dock,
                scope(),
                Some(operation()),
                || async { report() }
            ),
            Err(DisplayReject::RevisionChanged)
        ));
        let mut op = operation();
        op.expected_revision = 1;
        let task = gate
            .start_write(DisplayMutationKind::Dock, scope(), Some(op), || async {
                let mut r = report();
                r.status = MutationStatus::Unknown {
                    reason: "timeout".into(),
                };
                r
            })
            .unwrap();
        task.await.unwrap();
        gate.consume(&scope(), 1).unwrap();
        gate.bind_scope(scope()).unwrap();
        assert!(gate.snapshot().blocked.is_some());
        assert!(matches!(
            gate.start_write(DisplayMutationKind::ManualLayout, scope(), None, || async {
                report()
            }),
            Err(DisplayReject::Degraded)
        ));
    }
    #[tokio::test]
    async fn agent_duplicate_receipt_never_executes_twice() {
        let gate = Arc::new(AgentDisplayGate::new("agent".into()));
        gate.bind_scope(scope()).unwrap();
        gate.start_write(
            DisplayMutationKind::Dock,
            scope(),
            Some(operation()),
            || async { report() },
        )
        .unwrap()
        .await
        .unwrap();
        let h = gate
            .start_write(
                DisplayMutationKind::Dock,
                scope(),
                Some(operation()),
                || async { panic!("duplicate executed") },
            )
            .unwrap();
        h.await.unwrap();
        assert_eq!(gate.snapshot().revision, 1);
        let mut changed = operation();
        changed.scale = 2.;
        assert!(matches!(
            gate.start_write(
                DisplayMutationKind::Dock,
                scope(),
                Some(changed),
                || async { report() }
            ),
            Err(DisplayReject::InvalidOperation)
        ));
    }
}
