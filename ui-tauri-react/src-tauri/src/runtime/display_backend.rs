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
        Err(reason)
            if closed_laptop_panel_refusal(&reason)
                && report.before == report.after
                && !matches!(report.before, PanelObservation::Unknown { .. })
                && report.primary_enabled == Some(true) =>
        {
            MutationStatus::KnownFailure { reason }
        }
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

pub(crate) fn classify_manual_layout(
    command: &Result<(), String>,
    previous: &DisplayLayout,
    actual: Result<&DisplayLayout, &str>,
    requested: &DisplayLayout,
) -> MutationStatus {
    let Ok(actual) = actual else {
        return MutationStatus::Unknown {
            reason: "Requested display layout could not be verified".into(),
        };
    };
    if command.is_ok() && request_matches(requested, actual) {
        return MutationStatus::Verified;
    }
    if let Err(reason) = command {
        if layouts_unchanged(previous, actual) {
            return MutationStatus::KnownFailure {
                reason: reason.clone(),
            };
        }
    }
    MutationStatus::Unknown {
        reason: "Requested display layout could not be verified".into(),
    }
}

fn request_matches(requested: &DisplayLayout, actual: &DisplayLayout) -> bool {
    requested.displays.iter().all(|want| {
        actual.displays.iter().any(|display| {
            display.connector == want.connector
                && display.enabled == want.enabled
                && (!want.enabled
                    || (display.transform == want.transform
                        && (display.scale - want.scale).abs() < 0.02))
        })
    })
}

fn layouts_unchanged(previous: &DisplayLayout, actual: &DisplayLayout) -> bool {
    if previous.displays.len() != actual.displays.len() {
        return false;
    }
    previous.displays.iter().all(|old| {
        actual.displays.iter().any(|display| {
            display.connector == old.connector
                && display.enabled == old.enabled
                && display.primary == old.primary
                && display.transform == old.transform
                && display.x == old.x
                && display.y == old.y
                && display.current_mode.mode_id == old.current_mode.mode_id
                && (display.scale - old.scale).abs() < 0.02
        })
    })
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
        let command_status = match &result {
            Ok(()) => Ok(()),
            Err(reason) => Err(reason.clone()),
        };
        let after_layout = display_config::get_display_layout();
        let mut report = confirmed_report(
            before,
            result,
            sysfs::observe_secondary_panel(Path::new("/sys/class/drm")),
            after_layout.clone(),
            None,
        );
        match &mutation {
            ManualDisplayMutation::Layout { layout } => {
                report.status = classify_manual_layout(
                    &command_status,
                    &previous,
                    after_layout.as_ref().map_err(|error| error.as_str()),
                    layout,
                );
            }
            ManualDisplayMutation::Orientation { orientation, .. } => {
                let verified = after_layout.as_ref().is_ok_and(|actual| {
                    let transform = match orientation {
                        crate::models::Orientation::Normal => 0,
                        crate::models::Orientation::Left => 270,
                        crate::models::Orientation::Right => 90,
                        crate::models::Orientation::Inverted => 180,
                    };
                    actual.displays.iter().any(|display| {
                        display.connector == "eDP-1"
                            && display.enabled
                            && display.transform == transform
                    }) && previous.displays.iter().all(|old| {
                        actual.displays.iter().any(|display| {
                            display.connector == old.connector && display.enabled == old.enabled
                        })
                    })
                });
                if !verified {
                    report.status = MutationStatus::Unknown {
                        reason: "Requested display layout could not be verified".into(),
                    };
                }
            }
        }
        report
    })
    .await
    .unwrap_or_else(|_| MutationReport::unknown("Display mutation worker failed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DisplayInfo, DisplayMode, RefreshPolicy};

    fn sample_layout(secondary_scale: f64) -> DisplayLayout {
        DisplayLayout {
            displays: [("eDP-1", 1.666667, 0), ("eDP-2", secondary_scale, 1081)]
                .into_iter()
                .map(|(connector, scale, y)| DisplayInfo {
                    connector: connector.into(),
                    width: 2880,
                    height: 1800,
                    refresh_rate: 144.0,
                    scale,
                    x: 0,
                    y,
                    transform: if connector == "eDP-1" { 180 } else { 0 },
                    primary: connector == "eDP-1",
                    enabled: true,
                    current_mode: DisplayMode {
                        mode_id: "2880x1800@144".into(),
                        width: 2880,
                        height: 1800,
                        refresh_rate: 144.0,
                    },
                    available_modes: vec![],
                    refresh_policy: RefreshPolicy::Fixed,
                    supports_dynamic_refresh: false,
                })
                .collect(),
        }
    }

    #[test]
    fn closed_laptop_panel_refusal_with_unchanged_panels_is_known_failure() {
        let mut layout = sample_layout(1.666667);
        layout
            .displays
            .iter_mut()
            .find(|display| display.connector == "eDP-2")
            .unwrap()
            .enabled = false;
        let report = confirmed_report(
            PanelObservation::Disabled,
            Err("Failed to apply configuration: Refusing to activate a closed laptop panel".into()),
            PanelObservation::Disabled,
            Ok(layout),
            Some(true),
        );
        assert_eq!(
            report.status,
            MutationStatus::KnownFailure {
                reason: "Failed to apply configuration: Refusing to activate a closed laptop panel"
                    .into()
            }
        );
    }

    #[test]
    fn closed_laptop_panel_refusal_stays_unknown_when_panel_changes_or_reason_differs() {
        let mut layout = sample_layout(1.666667);
        layout
            .displays
            .iter_mut()
            .find(|display| display.connector == "eDP-2")
            .unwrap()
            .enabled = false;
        let changed = confirmed_report(
            PanelObservation::Disabled,
            Err("Failed to apply configuration: Refusing to activate a closed laptop panel".into()),
            PanelObservation::Enabled,
            Ok(layout.clone()),
            Some(true),
        );
        let other = confirmed_report(
            PanelObservation::Disabled,
            Err("gdctl set failed: something else".into()),
            PanelObservation::Disabled,
            Ok(layout),
            Some(true),
        );
        assert!(matches!(changed.status, MutationStatus::Unknown { .. }));
        assert!(matches!(other.status, MutationStatus::Unknown { .. }));
    }

    #[test]
    fn manual_layout_known_failure_when_readback_unchanged() {
        let previous = sample_layout(1.666667);
        let requested = sample_layout(1.5);
        let status = classify_manual_layout(
            &Err("gdctl set failed: scale rejected".into()),
            &previous,
            Ok(&previous),
            &requested,
        );
        assert_eq!(
            status,
            MutationStatus::KnownFailure {
                reason: "gdctl set failed: scale rejected".into()
            }
        );
    }

    #[test]
    fn manual_layout_known_failure_not_when_readback_changed() {
        let previous = sample_layout(1.666667);
        let mut changed = previous.clone();
        changed.displays[1].scale = 1.5;
        let status = classify_manual_layout(
            &Err("gdctl set failed: partial".into()),
            &previous,
            Ok(&changed),
            &changed,
        );
        assert!(matches!(status, MutationStatus::Unknown { .. }));
    }

    #[test]
    fn manual_layout_known_failure_not_when_success_unmatched() {
        let previous = sample_layout(1.666667);
        let status = classify_manual_layout(&Ok(()), &previous, Ok(&previous), &sample_layout(1.5));
        assert!(matches!(status, MutationStatus::Unknown { .. }));
    }

    #[test]
    fn manual_layout_known_failure_not_when_readback_missing() {
        let previous = sample_layout(1.666667);
        let status = classify_manual_layout(
            &Err("gdctl set failed: timeout".into()),
            &previous,
            Err("gdctl show failed"),
            &sample_layout(1.5),
        );
        assert!(matches!(status, MutationStatus::Unknown { .. }));
    }

    #[test]
    fn manual_layout_known_failure_not_used_for_matching_success() {
        let requested = sample_layout(1.5);
        let status = classify_manual_layout(
            &Ok(()),
            &sample_layout(1.666667),
            Ok(&requested),
            &requested,
        );
        assert_eq!(status, MutationStatus::Verified);
    }

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
