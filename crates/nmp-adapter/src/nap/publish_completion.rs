//! Terminal projection for one NAP publish request.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use nmp::Engine;
use nmp_native_nap_bridge::{ProviderPushSender, ProviderWriteCompletion, ProviderWriteRefusal};
use nmp_native_runtime_core::{BoundedJson, ReceiptEventSink, ReceiptSinkError, ReceiptSnapshot};
use serde_json::{Map, Value, json};

use super::{CachedEventLookup, NapDomain, cached_event_by_id, push_value};

const OVERSIZED_REFUSAL_ERROR: &str = "provider refusal exceeded configured response bound";

pub(super) struct NapPublishCompletion {
    domain: NapDomain,
    id: Arc<str>,
    outbound: ProviderPushSender,
    engine: Arc<Engine>,
    maximum_response_bytes: usize,
    receipt_event_lookup_timeout: Duration,
}

impl NapPublishCompletion {
    pub(super) fn new(
        domain: NapDomain,
        id: Arc<str>,
        outbound: ProviderPushSender,
        engine: Arc<Engine>,
        maximum_response_bytes: usize,
        receipt_event_lookup_timeout: Duration,
    ) -> Self {
        Self {
            domain,
            id,
            outbound,
            engine,
            maximum_response_bytes,
            receipt_event_lookup_timeout,
        }
    }

    fn into_sink(self) -> NapPublishReceiptSink {
        NapPublishReceiptSink::new(
            self.domain,
            self.id,
            self.outbound,
            self.engine,
            self.maximum_response_bytes,
            self.receipt_event_lookup_timeout,
        )
    }

    fn refusal_message(&self, reason: &str) -> Option<BoundedJson> {
        bounded_refusal_message(self.domain, &self.id, reason, self.maximum_response_bytes)
    }
}

fn bounded_refusal_message(
    domain: NapDomain,
    id: &str,
    reason: &str,
    maximum_response_bytes: usize,
) -> Option<BoundedJson> {
    let response = |error: &str| {
        BoundedJson::from_value(
            &json!({
                "type": format!("{}.publish.result", domain.name()),
                "id": id,
                "ok": false,
                "error": error,
            }),
            maximum_response_bytes,
        )
    };
    response(reason)
        .or_else(|_| response(OVERSIZED_REFUSAL_ERROR))
        .ok()
}

impl fmt::Debug for NapPublishCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NapPublishCompletion")
            .field("domain", &self.domain)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl ProviderWriteCompletion for NapPublishCompletion {
    fn into_receipt_sink(self: Box<Self>) -> Arc<dyn ReceiptEventSink> {
        Arc::new((*self).into_sink())
    }

    fn refused(self: Box<Self>, refusal: ProviderWriteRefusal) -> Option<BoundedJson> {
        match refusal {
            ProviderWriteRefusal::UserDenied(reason) => {
                (*self).into_sink().close(Some(reason));
                None
            }
            ProviderWriteRefusal::SystemUnavailable(reason) => self.refusal_message(&reason),
        }
    }
}

pub(super) struct NapPublishReceiptSink {
    domain: NapDomain,
    id: Arc<str>,
    outbound: ProviderPushSender,
    engine: Arc<Engine>,
    maximum_response_bytes: usize,
    receipt_event_lookup_timeout: Duration,
    delivered: AtomicBool,
}

impl NapPublishReceiptSink {
    pub(super) fn new(
        domain: NapDomain,
        id: Arc<str>,
        outbound: ProviderPushSender,
        engine: Arc<Engine>,
        maximum_response_bytes: usize,
        receipt_event_lookup_timeout: Duration,
    ) -> Self {
        Self {
            domain,
            id,
            outbound,
            engine,
            maximum_response_bytes,
            receipt_event_lookup_timeout,
            delivered: AtomicBool::new(false),
        }
    }
}

impl fmt::Debug for NapPublishReceiptSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NapPublishReceiptSink")
            .field("domain", &self.domain)
            .field("id", &self.id)
            .field("delivered", &self.delivered.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl ReceiptEventSink for NapPublishReceiptSink {
    fn push_latest(&self, snapshot: ReceiptSnapshot) -> Result<(), ReceiptSinkError> {
        if self.delivered.load(Ordering::Acquire) {
            return Ok(());
        }
        let value = snapshot
            .state
            .decode()
            .map_err(|_| ReceiptSinkError::FrameTooLarge)?;
        let state = value
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let terminal = matches!(
            state,
            "delivered"
                | "partial_delivery"
                | "exhausted"
                | "failed"
                | "cancelled"
                | "replaceable_conflict"
        );
        if !terminal {
            return Ok(());
        }
        if self.delivered.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let event_id = value
            .get("eventId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let relays = value
            .get("relays")
            .and_then(Value::as_object)
            .map(|relays| {
                relays
                    .iter()
                    .map(|(relay, result)| {
                        (
                            relay.clone(),
                            Value::Bool(
                                result.get("state").and_then(Value::as_str) == Some("acked"),
                            ),
                        )
                    })
                    .collect::<Map<_, _>>()
            })
            .unwrap_or_default();
        let mut ok = relays.values().any(|value| value == &Value::Bool(true));
        let mut response = json!({
            "type": format!("{}.publish.result", self.domain.name()),
            "id": self.id,
            "receiptId": snapshot.receipt_id.0,
            "ok": ok,
            "relays": relays,
        });
        if let Some(event_id) = &event_id {
            response["eventId"] = Value::String(event_id.clone());
            match cached_event_by_id(&self.engine, event_id, self.receipt_event_lookup_timeout) {
                CachedEventLookup::Found(event) => {
                    response["event"] = serde_json::to_value(event).unwrap_or(Value::Null);
                }
                CachedEventLookup::NotFound if self.domain == NapDomain::Relay && ok => {
                    // This cache read completed and confirmed absence, so a
                    // relay acknowledgement cannot be reported as success.
                    ok = false;
                    response["ok"] = Value::Bool(false);
                    response["error"] = Value::String(
                        "signed event was not readable from NMP canonical state".to_owned(),
                    );
                }
                CachedEventLookup::Unavailable if self.domain == NapDomain::Relay && ok => {
                    // An unresolved cache read is not evidence that the relay
                    // acknowledgement failed.
                    response["eventCacheReadTimedOut"] = Value::Bool(true);
                }
                CachedEventLookup::NotFound | CachedEventLookup::Unavailable => {}
            }
        }
        if !ok && response.get("error").is_none() {
            response["error"] = Value::String(
                value
                    .get("failure")
                    .and_then(Value::as_str)
                    .unwrap_or("NMP delivery did not receive a relay acknowledgement")
                    .to_owned(),
            );
        }
        push_value(
            &self.outbound,
            response,
            self.maximum_response_bytes,
            Some(&self.id),
        )
        .map(|_| ())
        .map_err(|_| ReceiptSinkError::Closed)
    }

    fn close(&self, reason: Option<Arc<str>>) {
        if self.delivered.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = push_value(
            &self.outbound,
            json!({
                "type": format!("{}.publish.result", self.domain.name()),
                "id": self.id,
                "ok": false,
                "error": reason.as_deref().unwrap_or("NMP receipt observation closed"),
            }),
            self.maximum_response_bytes,
            Some(&self.id),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_system_refusal_keeps_a_typed_bounded_result() {
        let fallback = bounded_refusal_message(
            NapDomain::Outbox,
            "request-1",
            OVERSIZED_REFUSAL_ERROR,
            usize::MAX,
        )
        .expect("fallback is serializable");

        let response = bounded_refusal_message(
            NapDomain::Outbox,
            "request-1",
            &"upstream failure ".repeat(128),
            fallback.byte_len(),
        )
        .expect("validated response bound retains the typed fallback");

        let value = response.decode().expect("fallback is valid JSON");
        assert_eq!(value["type"], "outbox.publish.result");
        assert_eq!(value["id"], "request-1");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"], OVERSIZED_REFUSAL_ERROR);
    }
}
