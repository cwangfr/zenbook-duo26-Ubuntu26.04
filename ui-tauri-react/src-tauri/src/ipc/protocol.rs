use crate::runtime::display_safety::{
    DisplayExecutionSnapshot, DisplayScope, DockOperation, DockReceipt, MutationReport,
};
use serde::{Deserialize, Serialize};

use crate::commands::usb_media_remap::UsbMediaRemapStatus;
use crate::models::{
    BatteryStatus, DisplayLayout, DuoSettings, DuoStatus, HardwareEvent, Orientation,
    PerformanceMode,
};

pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope<T> {
    pub protocol_version: u32,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version == PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(format!(
                "Protocol mismatch: expected {PROTOCOL_VERSION}, got {}",
                self.protocol_version
            ))
        }
    }
    pub fn new(payload: T) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonRequest {
    Ping,
    HandleLifecycle {
        phase: LifecyclePhase,
    },
    GetStatus,
    GetBatteryStatus,
    GetDisplayLayout,
    GetSettings,
    SaveSettings {
        settings: DuoSettings,
    },
    SetBacklight {
        level: u8,
    },
    SetDisplayBrightness {
        percent: u8,
    },
    SetChargeLimit {
        limit: u8,
    },
    ApplyPerformanceMode {
        mode: PerformanceMode,
    },
    SetOrientation {
        orientation: Orientation,
    },
    ApplyDisplayLayout {
        layout: DisplayLayout,
    },
    UsbMediaRemapStatus,
    UsbMediaRemapStart,
    UsbMediaRemapStop,
    UsbMediaRemapTogglePause,
    RestartService,
    RegisterSessionAgent {
        agent_id: String,
        session_id: String,
        backend: SessionBackend,
        socket_path: String,
    },
    TailLogs {
        lines: usize,
    },
    ClearLogs,
    GetRecentEvents {
        limit: usize,
    },
    ListTouchscreens,
    SetTouchscreenEnabled {
        connector: String,
        enabled: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResponse {
    SessionRegistered {
        scope: DisplayScope,
    },
    Pong,
    Ack,
    Status {
        status: DuoStatus,
    },
    BatteryStatus {
        status: BatteryStatus,
    },
    DisplayLayout {
        layout: DisplayLayout,
    },
    Settings {
        settings: DuoSettings,
    },
    UsbMediaRemapStatus {
        status: UsbMediaRemapStatus,
    },
    Logs {
        lines: Vec<String>,
    },
    Events {
        events: Vec<HardwareEvent>,
    },
    Touchscreens {
        devices: Vec<crate::hardware::touchscreen::TouchscreenDevice>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    Pre,
    Post,
    Hibernate,
    Thaw,
    Boot,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    StatusChanged {
        status: DuoStatus,
    },
    HardwareEvent {
        event: HardwareEvent,
    },
    SessionAgentChanged {
        connected: bool,
        backend: Option<SessionBackend>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionBackend {
    Gnome,
    Kde,
    Niri,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionCommand {
    ApplyDisplayMutation {
        scope: DisplayScope,
        mutation: ManualDisplayMutation,
    },
    BindDisplayScope {
        scope: DisplayScope,
        automatic_blocked: bool,
        manual_allowed: bool,
    },
    ApplyDockOperation {
        operation: DockOperation,
    },
    GetDisplayExecutionState,
    ConsumeDockReceipt {
        scope: DisplayScope,
        operation_id: u64,
    },
    GetDisplayLayout,
    SetDockMode {
        attached: bool,
        scale: f64,
    },
    ApplyDisplayLayout {
        layout: DisplayLayout,
    },
    SetOrientation {
        orientation: Orientation,
        scale: f64,
    },
    ShowNotification {
        title: String,
        message: String,
        urgent: bool,
    },
    OpenEmojiPicker,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionResponse {
    DockReceipt { receipt: DockReceipt },
    DisplayExecutionState { snapshot: DisplayExecutionSnapshot },
    MutationComplete { report: MutationReport },
    Ack,
    DisplayLayout { layout: DisplayLayout },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_wire_version_is_rejected() {
        let old = Envelope {
            protocol_version: 2,
            payload: DaemonRequest::Ping,
        };
        assert!(old.validate().is_err());
    }
    #[test]
    fn scoped_receipt_roundtrip_keeps_identity_and_evidence() {
        use crate::runtime::display_safety::{
            tests::{operation, report},
            DockReceipt,
        };
        let value = SessionResponse::DockReceipt {
            receipt: DockReceipt {
                operation: operation(),
                report: report(),
            },
        };
        let raw = serde_json::to_string(&Envelope::new(value)).unwrap();
        let parsed: Envelope<SessionResponse> = serde_json::from_str(&raw).unwrap();
        parsed.validate().unwrap();
        let SessionResponse::DockReceipt { receipt } = parsed.payload else {
            panic!("wrong response")
        };
        assert_eq!(receipt.operation.scope.agent_id, "agent");
        assert_eq!(receipt.operation.target_generation, 1);
        assert_eq!(receipt.report.secondary_enabled, Some(false));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ManualDisplayMutation {
    Layout {
        layout: DisplayLayout,
    },
    Orientation {
        orientation: Orientation,
        scale: f64,
    },
}
