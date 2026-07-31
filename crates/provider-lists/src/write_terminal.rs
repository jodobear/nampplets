//! Minimum bounded terminal shapes for native-mediated list writes.

use serde_json::{Map, Value};

use crate::write::ListsAction;

pub(crate) const USER_DENIED_ERROR: &str = "user-denied";
pub(crate) const LIST_UNAVAILABLE_ERROR: &str = "list-unavailable";
pub(crate) const PUBLISH_FAILED_ERROR: &str = "publish-failed";
const TERMINAL_ERRORS: [&str; 3] = [
    USER_DENIED_ERROR,
    LIST_UNAVAILABLE_ERROR,
    PUBLISH_FAILED_ERROR,
];

pub(crate) fn terminal_fallback(
    action: ListsAction,
    id: &str,
    ok: bool,
    changed: usize,
    skipped: usize,
    error: Option<&str>,
) -> Value {
    let mut envelope = Map::new();
    envelope.insert("type".to_owned(), Value::from(action.result_type()));
    envelope.insert("id".to_owned(), Value::from(id));
    envelope.insert("ok".to_owned(), Value::from(ok));
    envelope.insert(
        action.changed_field().to_owned(),
        Value::from(if ok { changed } else { 0 }),
    );
    envelope.insert(
        "skipped".to_owned(),
        Value::from(if ok { skipped } else { 0 }),
    );
    if let Some(error) = error {
        envelope.insert("error".to_owned(), Value::from(error));
    }
    Value::Object(envelope)
}

pub(crate) fn minimum_terminal_response_bytes(
    maximum_correlation_id_bytes: usize,
    maximum_request_items: usize,
) -> Option<usize> {
    // One input byte can become six JSON bytes when it is an escaped control.
    let escaped_id_bytes = maximum_correlation_id_bytes.checked_mul(6)?;
    let mut largest = 0;
    for action in [ListsAction::Add, ListsAction::Remove] {
        let success = terminal_fallback(
            action,
            "",
            true,
            maximum_request_items,
            maximum_request_items,
            None,
        );
        largest = largest.max(serde_json::to_vec(&success).ok()?.len());
        for error in TERMINAL_ERRORS {
            let failure = terminal_fallback(action, "", false, 0, 0, Some(error));
            largest = largest.max(serde_json::to_vec(&failure).ok()?.len());
        }
    }
    largest.checked_add(escaped_id_bytes)
}
