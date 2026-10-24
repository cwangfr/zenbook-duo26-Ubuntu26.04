//! Readback evidence for the Agent's serialized display mutations.
use super::display_safety::*;
use crate::hardware::{display_config, sysfs};
use crate::ipc::protocol::ManualDisplayMutation;
use crate::models::DisplayLayout;
use std::path::Path;

pub(crate) fn confirmed_report(
    before: PanelObservation,
    result: Result<(), String>,
    after: PanelObservation,
    layout: Result<DisplayLayout, String>,
    attached: Option<bool>,
) -> MutationReport {
    let mut report = MutationReport::unknown("Display outcome not verified");
    report.before = before;
    report.after = after;
    match layout {
        Ok(layout) => {
            report.primary_enabled = Some(
                layout
                    .displays
                    .iter()
                    .any(|d| d.connector == "eDP-1" && d.enabled),
            );
            report.secondary_enabled = Some(
                layout
                    .displays
                    .iter()
                    .any(|d| d.connector == "eDP-2" && d.enabled),
            );
        }
        Err(reason) => {
            report.status = MutationStatus::Unknown { reason };
            return report;
        }
    }
    report.status = match result {
        Err(reason) => MutationStatus::Unknown { reason },
        Ok(())
            if !matches!(report.before, PanelObservation::Unknown { .. })
                && !matches!(report.after, PanelObservation::Unknown { .. })
                && report.primary_enabled == Some(true)
                && attached.is_none_or(|a| {
                    report.secondary_enabled == Some(!a)
                        && report.after
                            == if a {
                                PanelObservation::Disabled
                            } else {
                                PanelObservation::Enabled
                            }
                }) =>
        {
            MutationStatus::Verified
        }
        _ => MutationStatus::Unknown {
            reason: "Display verification did not match the operation".into(),
        },
    };
    report
}

pub(crate) async fn run_dock(operation: DockOperation) -> MutationReport {
    let before = sysfs::observe_secondary_panel(Path::new("/sys/class/drm"));
    if !automatic_dock_allowed(
        operation.attached,
        &before,
        operation.ownership_authorized,
        false,
    ) {
        return MutationReport::not_started("Automatic display protection blocked the operation");
    }
    let result = super::session_agent::apply_dock_mode(operation.attached, operation.scale).await;
    tokio::task::spawn_blocking(move || {
        confirmed_report(
            before,
            result,
            sysfs::observe_secondary_panel(Path::new("/sys/class/drm")),
            display_config::get_display_layout(),
            Some(operation.attached),
        )
    })
    .await
    .unwrap_or_else(|_| MutationReport::unknown("Display readback worker failed"))
}

pub(crate) async fn run_manual(mutation: ManualDisplayMutation, sensor: bool) -> MutationReport {
    tokio::task::spawn_blocking(move || {
        let before = sysfs::observe_secondary_panel(Path::new("/sys/class/drm"));
        let previous = match display_config::get_display_layout() {
            Ok(l) => l,
            Err(e) => return MutationReport::not_started(e),
        };
        if matches!(before, PanelObservation::Unknown { .. }) {
            return MutationReport::not_started("Cannot observe secondary display");
        }
        if sensor
            && !["eDP-1", "eDP-2"].iter().all(|c| {
                previous
                    .displays
                    .iter()
                    .any(|d| &d.connector == c && d.enabled)
            })
        {
            return MutationReport::not_started("Automatic rotation requires two active panels");
        }
        let result = match &mutation {
            ManualDisplayMutation::Layout { layout } => {
                display_config::apply_display_layout(layout)
            }
            ManualDisplayMutation::Orientation { orientation, scale } => {
                display_config::set_orientation_with_scale(orientation, *scale)
            }
        };
        let after_layout = display_config::get_display_layout();
        let verified = after_layout.as_ref().is_ok_and(|actual| match &mutation {
            ManualDisplayMutation::Layout { layout } => layout.displays.iter().all(|want| {
                actual.displays.iter().any(|d| {
                    d.connector == want.connector
                        && d.enabled == want.enabled
                        && (!want.enabled
                            || (d.transform == want.transform
                                && (d.scale - want.scale).abs() < 0.02))
                })
            }),
            ManualDisplayMutation::Orientation { orientation, .. } => {
                let transform = match orientation {
                    crate::models::Orientation::Normal => 180,
                    crate::models::Orientation::Left => 90,
                    crate::models::Orientation::Right => 270,
                    crate::models::Orientation::Inverted => 0,
                };
                actual
                    .displays
                    .iter()
                    .any(|d| d.connector == "eDP-1" && d.enabled && d.transform == transform)
                    && previous.displays.iter().all(|old| {
                        actual
                            .displays
                            .iter()
                            .any(|d| d.connector == old.connector && d.enabled == old.enabled)
                    })
            }
        });
        let mut report = confirmed_report(
            before,
            result,
            sysfs::observe_secondary_panel(Path::new("/sys/class/drm")),
            after_layout,
            None,
        );
        if !verified {
            report.status = MutationStatus::Unknown {
                reason: "Requested display layout could not be verified".into(),
            };
        }
        report
    })
    .await
    .unwrap_or_else(|_| MutationReport::unknown("Display mutation worker failed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readback_failure_after_command_success_is_unknown() {
        let r = confirmed_report(
            PanelObservation::Enabled,
            Ok(()),
            PanelObservation::Disabled,
            Err("readback failed".into()),
            Some(true),
        );
        assert!(matches!(r.status, MutationStatus::Unknown { .. }));
        let mut receipt = DockReceipt {
            operation: super::super::display_safety::tests::operation(),
            report: r,
        };
        assert_eq!(
            classify_dock_commit(&receipt.operation.scope, 1, &receipt),
            CommitDecision::Unknown
        );
        receipt.report = confirmed_report(
            PanelObservation::Enabled,
            Err("timed out".into()),
            PanelObservation::Disabled,
            Ok(DisplayLayout { displays: vec![] }),
            Some(true),
        );
        assert!(matches!(
            receipt.report.status,
            MutationStatus::Unknown { .. }
        ));
    }
}
