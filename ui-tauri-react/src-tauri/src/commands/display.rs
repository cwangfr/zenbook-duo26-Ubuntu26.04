use crate::hardware::display_config;
use crate::ipc::protocol::{DaemonRequest, DaemonResponse};
use crate::models::{DisplayLayout, Orientation};
use crate::runtime::client;

#[tauri::command]
pub fn get_display_layout() -> Result<DisplayLayout, String> {
    match client::request(DaemonRequest::GetDisplayLayout) {
        Ok(DaemonResponse::DisplayLayout { layout }) => {
            Ok(display_config::normalize_display_layout(layout))
        }
        Ok(DaemonResponse::Error { .. }) => display_config::get_display_layout(),
        Ok(_) => display_config::get_display_layout(),
        Err(_) => display_config::get_display_layout(),
    }
}

#[tauri::command]
pub fn apply_display_layout(layout: DisplayLayout) -> Result<(), String> {
    let normalized = display_config::normalize_display_layout(layout);

    mutation_result(client::request(DaemonRequest::ApplyDisplayLayout {
        layout: normalized,
    }))
}

#[tauri::command]
pub fn set_orientation(orientation: Orientation) -> Result<(), String> {
    mutation_result(client::request(DaemonRequest::SetOrientation {
        orientation,
    }))
}

pub(crate) fn mutation_result(response: Result<DaemonResponse, String>) -> Result<(), String> {
    match response? {
        DaemonResponse::Ack => Ok(()),
        DaemonResponse::Error { message } => Err(message),
        _ => Err("Unexpected display mutation response".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_or_unreachable_daemon_never_becomes_mutation_success() {
        assert!(mutation_result(Err("socket unavailable".into())).is_err());
        assert!(mutation_result(Ok(DaemonResponse::Error {
            message: "display protection".into()
        }))
        .is_err());
        assert!(mutation_result(Ok(DaemonResponse::Pong)).is_err());
        assert!(mutation_result(Ok(DaemonResponse::Ack)).is_ok());
    }
}
