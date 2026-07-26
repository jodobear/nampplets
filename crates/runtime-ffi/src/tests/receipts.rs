use std::sync::Arc;

use nmp_native_runtime_app::{ReceiptDeliveryState, ReceiptView};
use nmp_native_runtime_core::{BoundedJson, ReceiptSnapshot, WriteReceiptId};

use crate::{RuntimeReceiptObservationLifecycle, RuntimeReceiptOutcome, project_receipt};

const ID: &str = "receipt-42";

fn relay(state: &str) -> serde_json::Value {
    serde_json::json!({"state": state, "terminal": true})
}

fn state(name: &str, relays: serde_json::Value) -> String {
    serde_json::json!({
        "schema": "nostr.write.receipt/1",
        "state": name,
        "relays": relays,
    })
    .to_string()
}

fn view(raw: Option<String>, delivery: ReceiptDeliveryState) -> ReceiptView {
    ReceiptView {
        receipt_id: WriteReceiptId(Arc::from(ID)),
        delivery,
        latest: raw.map(|raw| ReceiptSnapshot {
            receipt_id: WriteReceiptId(Arc::from(ID)),
            state: BoundedJson::from_raw(raw, 64 * 1_024).unwrap(),
        }),
    }
}

fn outcome(raw: String) -> RuntimeReceiptOutcome {
    project_receipt(&view(Some(raw), ReceiptDeliveryState::Observing)).outcome
}

#[test]
fn only_a_coherent_canonical_delivered_state_is_delivered() {
    let delivered = state(
        "delivered",
        serde_json::json!({"wss://one.example": relay("acked")}),
    );
    assert_eq!(outcome(delivered), RuntimeReceiptOutcome::Delivered);

    let contradictory = state(
        "delivered",
        serde_json::json!({"wss://one.example": relay("rejected")}),
    );
    assert_eq!(outcome(contradictory), RuntimeReceiptOutcome::Unavailable);
    assert_eq!(
        outcome(state(
            "future_delivered",
            serde_json::json!({"wss://one.example": relay("acked")}),
        )),
        RuntimeReceiptOutcome::Unavailable
    );
}

#[test]
fn nonterminal_and_every_terminal_family_remain_distinct() {
    let cases = [
        (
            state("accepted", serde_json::json!({})),
            RuntimeReceiptOutcome::InProgress,
        ),
        (
            state(
                "partial_delivery",
                serde_json::json!({
                    "wss://one.example": relay("acked"),
                    "wss://two.example": relay("rejected"),
                }),
            ),
            RuntimeReceiptOutcome::PartialDelivery,
        ),
        (
            state(
                "exhausted",
                serde_json::json!({"wss://one.example": relay("gave_up")}),
            ),
            RuntimeReceiptOutcome::Exhausted,
        ),
        (
            state(
                "exhausted",
                serde_json::json!({"wss://one.example": relay("outcome_unknown")}),
            ),
            RuntimeReceiptOutcome::Ambiguous,
        ),
        (
            state(
                "exhausted",
                serde_json::json!({"wss://one.example": relay("rejected")}),
            ),
            RuntimeReceiptOutcome::Refused,
        ),
        (
            serde_json::json!({
                "schema": "nostr.write.receipt/1",
                "state": "failed",
                "failure": "signer refused",
                "relays": {},
            })
            .to_string(),
            RuntimeReceiptOutcome::Failed,
        ),
        (
            state("cancelled", serde_json::json!({})),
            RuntimeReceiptOutcome::Cancelled,
        ),
        (
            serde_json::json!({
                "schema": "nostr.write.receipt/1",
                "state": "replaceable_conflict",
                "conflict": {"expected": "a", "actual": "b"},
                "relays": {},
            })
            .to_string(),
            RuntimeReceiptOutcome::Conflict,
        ),
    ];
    for (raw, expected) in cases {
        assert_eq!(outcome(raw), expected);
    }
}

#[test]
fn absent_malformed_unknown_and_oversized_states_are_unavailable() {
    let absent = project_receipt(&view(None, ReceiptDeliveryState::Observing));
    assert_eq!(absent.outcome, RuntimeReceiptOutcome::Unavailable);

    for raw in [
        "[]".to_owned(),
        serde_json::json!({"schema": "future", "state": "delivered"}).to_string(),
        serde_json::json!({"schema": "nostr.write.receipt/1"}).to_string(),
        state("future", serde_json::json!({})),
    ] {
        assert_eq!(outcome(raw), RuntimeReceiptOutcome::Unavailable);
    }

    let oversized = serde_json::json!({
        "schema": "nostr.write.receipt/1",
        "state": "failed",
        "failure": "x".repeat(17 * 1_024),
        "relays": {},
    })
    .to_string();
    let projected = project_receipt(&view(
        Some(oversized.clone()),
        ReceiptDeliveryState::Observing,
    ));
    assert_eq!(projected.outcome, RuntimeReceiptOutcome::Unavailable);
    assert_eq!(
        projected.latest_state_json.as_deref(),
        Some(oversized.as_str())
    );
}

#[test]
fn lifecycle_is_typed_and_close_cannot_weaken_a_durable_outcome() {
    let raw = state(
        "delivered",
        serde_json::json!({"wss://one.example": relay("acked")}),
    );
    let observing = project_receipt(&view(Some(raw.clone()), ReceiptDeliveryState::Observing));
    let closed = project_receipt(&view(Some(raw), ReceiptDeliveryState::Closed));
    assert_eq!(observing.outcome, RuntimeReceiptOutcome::Delivered);
    assert_eq!(closed.outcome, RuntimeReceiptOutcome::Delivered);
    assert_eq!(
        observing.observation_lifecycle,
        RuntimeReceiptObservationLifecycle::Observing
    );
    assert_eq!(
        closed.observation_lifecycle,
        RuntimeReceiptObservationLifecycle::Closed
    );

    let not_found = project_receipt(&view(None, ReceiptDeliveryState::NotFound));
    assert_eq!(
        not_found.observation_lifecycle,
        RuntimeReceiptObservationLifecycle::NotFound
    );
    assert_eq!(not_found.outcome, RuntimeReceiptOutcome::Unavailable);
}
