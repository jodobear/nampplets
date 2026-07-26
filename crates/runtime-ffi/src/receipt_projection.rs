//! Mechanical projection of canonical NMP receipt evidence.
//!
//! Observation lifecycle is intentionally independent: closing a consumer can
//! never turn an absent outcome into success or weaken a retained terminal
//! outcome. Raw canonical JSON remains available as evidence, but native code
//! receives the exhaustive Rust classification and never parses it.

use nmp_native_runtime_app::{ReceiptDeliveryState, ReceiptView};
use serde_json::{Map, Value};

use crate::{RuntimeReceiptObservationLifecycle, RuntimeReceiptOutcome, RuntimeReceiptSnapshot};

const RECEIPT_SCHEMA: &str = "nostr.write.receipt/1";
const MAXIMUM_CANONICAL_RECEIPT_BYTES: usize = 16 * 1_024;

pub fn project_receipt(receipt: &ReceiptView) -> RuntimeReceiptSnapshot {
    let latest_state_json = receipt
        .latest
        .as_ref()
        .map(|latest| latest.state.as_str().to_owned());
    let (outcome, outcome_detail) = classify(latest_state_json.as_deref());
    RuntimeReceiptSnapshot {
        receipt_id: receipt.receipt_id.0.to_string(),
        outcome,
        observation_lifecycle: match receipt.delivery {
            ReceiptDeliveryState::Observing => RuntimeReceiptObservationLifecycle::Observing,
            ReceiptDeliveryState::NotFound => RuntimeReceiptObservationLifecycle::NotFound,
            ReceiptDeliveryState::Closed => RuntimeReceiptObservationLifecycle::Closed,
        },
        outcome_detail,
        latest_state_json,
    }
}

fn classify(raw: Option<&str>) -> (RuntimeReceiptOutcome, Option<String>) {
    let Some(raw) = raw else {
        return unavailable("no canonical NMP receipt state is available");
    };
    if raw.len() > MAXIMUM_CANONICAL_RECEIPT_BYTES {
        return unavailable(format!(
            "canonical NMP receipt state is {} bytes; the maximum is \
             {MAXIMUM_CANONICAL_RECEIPT_BYTES}",
            raw.len()
        ));
    }
    let value: Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(error) => {
            return unavailable(format!("canonical NMP receipt state is malformed: {error}"));
        }
    };
    let Some(object) = value.as_object() else {
        return unavailable("canonical NMP receipt state is not an object");
    };
    if object.get("schema").and_then(Value::as_str) != Some(RECEIPT_SCHEMA) {
        return unavailable("canonical NMP receipt schema is missing or unrecognized");
    }
    let Some(state) = object.get("state").and_then(Value::as_str) else {
        return unavailable("canonical NMP receipt state name is missing");
    };
    match state {
        "observing" | "accepted" | "awaiting_capability" | "signed" | "delivering" => {
            (RuntimeReceiptOutcome::InProgress, None)
        }
        "delivered" => classify_terminal_relays(object, TerminalKind::Delivered),
        "partial_delivery" => classify_terminal_relays(object, TerminalKind::Partial),
        "exhausted" => classify_terminal_relays(object, TerminalKind::Exhausted),
        "failed" => (
            RuntimeReceiptOutcome::Failed,
            object
                .get("failure")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| Some("NMP reports a terminal write failure".to_owned())),
        ),
        "cancelled" => (
            RuntimeReceiptOutcome::Cancelled,
            Some("NMP reports that the write was cancelled".to_owned()),
        ),
        "replaceable_conflict" if object.get("conflict").is_some_and(Value::is_object) => (
            RuntimeReceiptOutcome::Conflict,
            Some("NMP reports a replaceable-event conflict".to_owned()),
        ),
        "replaceable_conflict" => unavailable("canonical replaceable-conflict evidence is missing"),
        other => unavailable(format!(
            "canonical NMP receipt state {other:?} is unrecognized"
        )),
    }
}

#[derive(Clone, Copy)]
enum TerminalKind {
    Delivered,
    Partial,
    Exhausted,
}

fn classify_terminal_relays(
    object: &Map<String, Value>,
    expected: TerminalKind,
) -> (RuntimeReceiptOutcome, Option<String>) {
    let Some(relays) = object.get("relays").and_then(Value::as_object) else {
        return unavailable("canonical terminal receipt has no relay evidence");
    };
    if relays.is_empty() {
        return unavailable("canonical terminal receipt has an empty relay set");
    }
    let mut acked = 0;
    let mut rejected = 0;
    let mut gave_up = 0;
    let mut unknown = 0;
    for relay in relays.values() {
        let Some(relay) = relay.as_object() else {
            return unavailable("canonical receipt relay evidence is malformed");
        };
        if relay.get("terminal").and_then(Value::as_bool) != Some(true) {
            return unavailable("canonical terminal receipt contains a nonterminal relay");
        }
        match relay.get("state").and_then(Value::as_str) {
            Some("acked") => acked += 1,
            Some("rejected") => rejected += 1,
            Some("gave_up") => gave_up += 1,
            Some("outcome_unknown") => unknown += 1,
            _ => return unavailable("canonical terminal receipt has an unrecognized relay state"),
        }
    }
    let total = relays.len();
    match expected {
        TerminalKind::Delivered if acked == total => (RuntimeReceiptOutcome::Delivered, None),
        TerminalKind::Partial if acked > 0 && acked < total => {
            (RuntimeReceiptOutcome::PartialDelivery, None)
        }
        TerminalKind::Exhausted if acked == 0 && unknown > 0 => (
            RuntimeReceiptOutcome::Ambiguous,
            Some("at least one relay attempt has an unknown terminal outcome".to_owned()),
        ),
        TerminalKind::Exhausted if rejected == total => (
            RuntimeReceiptOutcome::Refused,
            Some("every terminal relay outcome was rejected".to_owned()),
        ),
        TerminalKind::Exhausted if acked == 0 && rejected + gave_up == total => (
            RuntimeReceiptOutcome::Exhausted,
            Some("all relay attempts ended without an acknowledgement".to_owned()),
        ),
        _ => unavailable("canonical receipt state contradicts its relay evidence"),
    }
}

fn unavailable(detail: impl Into<String>) -> (RuntimeReceiptOutcome, Option<String>) {
    (RuntimeReceiptOutcome::Unavailable, Some(detail.into()))
}
