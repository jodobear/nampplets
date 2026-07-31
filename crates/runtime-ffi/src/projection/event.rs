//! Typed projection of ordered runtime application events.

use nmp_native_runtime_app::PlatformEvent;
use nmp_native_runtime_core::SessionState;

use crate::RuntimeEvent;

pub(crate) fn project_event(sequence: u64, event: &PlatformEvent) -> RuntimeEvent {
    let kind = match event {
        PlatformEvent::Installed { .. } => "installed",
        PlatformEvent::LibraryFilterChanged { .. } => "library-filter-changed",
        PlatformEvent::Uninstalled { .. } => "uninstalled",
        PlatformEvent::GrantChanged { .. } => "grant-changed",
        PlatformEvent::PermissionChangesApplied { .. } => "permission-changes-applied",
        PlatformEvent::SessionChanged(snapshot) => {
            let lifecycle = match snapshot.state {
                SessionState::Launching => "launching",
                SessionState::Running => "running",
                SessionState::Suspended => "suspended",
                SessionState::Crashed => "crashed",
                SessionState::Stopped => "stopped",
            };
            return RuntimeEvent {
                sequence,
                kind: "session-changed".to_owned(),
                detail: lifecycle.to_owned(),
                session_id: Some(snapshot.id.0),
                response_json: None,
            };
        }
        PlatformEvent::EnvelopeHandled {
            session, response, ..
        } => {
            return RuntimeEvent {
                sequence,
                kind: "envelope-handled".to_owned(),
                detail: format!("{event:?}"),
                session_id: Some(session.0),
                response_json: response.as_ref().map(|value| value.as_str().to_owned()),
            };
        }
        PlatformEvent::EnvelopeIgnored { .. } => "envelope-ignored",
        PlatformEvent::NappletDiagnostic {
            session,
            level,
            message,
        } => {
            // Projected structurally rather than as a debug string: the point
            // of classifying in Rust is that native renders a typed fact
            // instead of re-parsing one.
            return RuntimeEvent {
                sequence,
                kind: "napplet-diagnostic".to_owned(),
                detail: level.as_str().to_owned(),
                session_id: Some(session.0),
                response_json: Some(message.clone()),
            };
        }
        PlatformEvent::ProviderOperationFinished { .. } => "provider-operation-finished",
        PlatformEvent::ProviderPush {
            session, envelope, ..
        } => {
            return RuntimeEvent {
                sequence,
                kind: "provider-push".to_owned(),
                detail: format!("{event:?}"),
                session_id: Some(session.0),
                response_json: Some(envelope.as_str().to_owned()),
            };
        }
        PlatformEvent::ProviderPushLaneClosed { session, .. } => {
            return RuntimeEvent {
                sequence,
                kind: "provider-push-lane-closed".to_owned(),
                detail: format!("{event:?}"),
                session_id: Some(session.0),
                response_json: None,
            };
        }
        PlatformEvent::BindingOpened { .. } => "binding-opened",
        PlatformEvent::BindingClosed { .. } => "binding-closed",
        PlatformEvent::WriteAccepted { .. } => "write-accepted",
        PlatformEvent::WorkspaceSaved { .. } => "workspace-saved",
        PlatformEvent::WorkspaceRestored { .. } => "workspace-restored",
        PlatformEvent::WorkspaceAssignmentChanged { .. } => "workspace-assignment-changed",
        PlatformEvent::ReceiptReattached { .. } => "receipt-reattached",
        PlatformEvent::ReceiptNotFound { .. } => "receipt-not-found",
        PlatformEvent::Refused(_) => "refused",
        PlatformEvent::Closed => "closed",
    };
    RuntimeEvent {
        sequence,
        kind: kind.to_owned(),
        detail: format!("{event:?}"),
        session_id: None,
        response_json: None,
    }
}
