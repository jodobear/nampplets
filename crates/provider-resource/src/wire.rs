use std::{collections::BTreeMap, sync::Arc};

use nmp_native_nap_bridge::{ProviderError, ProviderRequest};
use nmp_native_runtime_core::{BoundedJson, Principal};
use serde_json::{Map, Value, json};

use crate::{DOMAIN, ResourceFailure, ResourceProviderLimits, provider::RateBucket};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceCensus {
    pub sessions: usize,
    pub active_requests: usize,
    pub in_flight_urls: usize,
    pub closed: bool,
}

pub(crate) fn take_rate_tokens(
    buckets: &mut BTreeMap<Principal, RateBucket>,
    principal: &Principal,
    now_millis: u64,
    count: usize,
    maximum_per_minute: u32,
) -> bool {
    let capacity = u64::from(maximum_per_minute).saturating_mul(1_000);
    let bucket = buckets.entry(principal.clone()).or_insert(RateBucket {
        tokens_milli: capacity,
        updated_at_millis: now_millis,
    });
    let elapsed = now_millis.saturating_sub(bucket.updated_at_millis);
    let refill = elapsed
        .saturating_mul(u64::from(maximum_per_minute))
        .saturating_div(60);
    bucket.tokens_milli = bucket.tokens_milli.saturating_add(refill).min(capacity);
    bucket.updated_at_millis = now_millis;
    let requested = u64::try_from(count)
        .unwrap_or(u64::MAX)
        .saturating_mul(1_000);
    if bucket.tokens_milli < requested {
        return false;
    }
    bucket.tokens_milli -= requested;
    true
}

pub(crate) fn decrement_principal(
    counts: &mut BTreeMap<Principal, usize>,
    principal: &Principal,
    amount: usize,
) {
    let Some(count) = counts.get_mut(principal) else {
        return;
    };
    *count = count.saturating_sub(amount);
    if *count == 0 {
        counts.remove(principal);
    }
}

pub(crate) fn exact_payload<'a>(
    request: &'a ProviderRequest,
    fields: &[&str],
) -> Result<&'a Map<String, Value>, ProviderError> {
    let payload = request
        .payload
        .as_object()
        .ok_or_else(|| invalid(request, "payload must be an object"))?;
    if payload.len() != fields.len() || fields.iter().any(|field| !payload.contains_key(*field)) {
        return Err(invalid(request, "payload fields do not match the action"));
    }
    Ok(payload)
}

pub(crate) fn required_string<'a>(
    payload: &'a Map<String, Value>,
    field: &str,
    request: &ProviderRequest,
) -> Result<&'a str, ProviderError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(request, format!("{field} must be a string")))
}

pub(crate) fn correlation_id(
    request: &ProviderRequest,
    limits: ResourceProviderLimits,
) -> Result<Arc<str>, ProviderError> {
    let id = request
        .correlation_id
        .as_ref()
        .ok_or_else(|| invalid(request, "id is required"))?;
    if id.is_empty() || id.len() > limits.maximum_correlation_id_bytes {
        return Err(invalid(
            request,
            format!(
                "id must be 1..={} bytes",
                limits.maximum_correlation_id_bytes
            ),
        ));
    }
    Ok(Arc::clone(id))
}

pub(crate) fn error_envelope(message_type: &str, id: &str, failure: &ResourceFailure) -> Value {
    json!({
        "type": message_type,
        "id": id,
        "error": failure.code.as_str(),
        "message": failure.message,
    })
}

pub(crate) fn bounded_response(
    response: &Value,
    maximum: usize,
    request: &ProviderRequest,
) -> Result<BoundedJson, ProviderError> {
    BoundedJson::from_value(response, maximum)
        .map_err(|_| failed(request, "resource response exceeded its native wire bound"))
}

pub(crate) fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

pub(crate) fn failed(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

pub(crate) fn lifecycle_error(reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(DOMAIN),
        action: Arc::from("session.lifecycle"),
        reason: reason.into(),
    }
}
