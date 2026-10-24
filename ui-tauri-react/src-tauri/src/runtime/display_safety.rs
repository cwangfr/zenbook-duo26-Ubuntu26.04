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
    pub pending: Option<DockOperation>,
    pub receipt_to_consume: Option<DockReceipt>,
    pub degraded: Option<String>,
    pub manual_allowed: bool,
    pub target: Option<DockTarget>,
    pub next_operation_id: u64,
    pub next_target_generation: u64,
    pub last_seen_revision: Option<u64>,
    #[serde(skip)]
    pub worker_running: bool,
}
impl Default for DisplaySafetyState {
    fn default() -> Self {
        Self {
            boot_id: String::new(),
            daemon_id: String::new(),
            lifecycle_epoch: 0,
            pending: None,
            receipt_to_consume: None,
            degraded: None,
            manual_allowed: true,
            target: None,
            next_operation_id: 0,
            next_target_generation: 0,
            last_seen_revision: None,
            worker_running: false,
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
        MutationStatus::Verified
            if receipt.report.primary_enabled == Some(true)
                && receipt.report.secondary_enabled == Some(!receipt.operation.attached) =>
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
        } else if self.pending.is_some() || self.receipt_to_consume.is_some() {
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
        r.report.status = MutationStatus::Verified;
        r.report.secondary_enabled = Some(true);
        assert_eq!(
            classify_dock_commit(&scope(), 1, &r),
            CommitDecision::Unknown
        );
    }
}
