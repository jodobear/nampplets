use std::sync::Arc;

use nmp_native_nap_bridge::{ProviderCall, ProviderError, ProviderRequest};
use nmp_native_runtime_core::BoundedJson;
use serde_json::{Map, Value, json};

use crate::{DOMAIN, IdentityDataError, IdentityProviderLimits, IdentityQuery, IdentityValue};

pub(crate) fn correlation_id(
    request: &ProviderRequest,
    limits: IdentityProviderLimits,
) -> Result<&str, ProviderError> {
    let id = request
        .correlation_id
        .as_deref()
        .ok_or_else(|| invalid_payload(request, "id is required"))?;
    if id.is_empty() || id.len() > limits.maximum_correlation_id_bytes {
        return Err(invalid_payload(
            request,
            format!(
                "id must be 1..={} bytes",
                limits.maximum_correlation_id_bytes
            ),
        ));
    }
    Ok(id)
}

pub(crate) fn exact_payload<'a>(
    request: &'a ProviderRequest,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, ProviderError> {
    let payload = request
        .payload
        .as_object()
        .ok_or_else(|| invalid_payload(request, "payload must be a flat object"))?;
    if payload.len() != allowed.len() || payload.keys().any(|key| !allowed.contains(&key.as_str()))
    {
        return Err(invalid_payload(
            request,
            format!("expected exactly these fields: {}", allowed.join(", ")),
        ));
    }
    Ok(payload)
}

pub(crate) fn required_string<'a>(
    payload: &'a Map<String, Value>,
    key: &str,
    request: &ProviderRequest,
) -> Result<&'a str, ProviderError> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_payload(request, format!("{key} must be a string")))
}

pub(crate) fn invalid_payload(
    request: &ProviderRequest,
    reason: impl Into<Arc<str>>,
) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

pub(crate) fn response(
    value: Value,
    limits: IdentityProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    let response =
        BoundedJson::from_value(&value, limits.maximum_response_bytes).map_err(|_| {
            ProviderError::Failed {
                domain: Arc::from(DOMAIN),
                action: Arc::clone(&request.action),
                reason: Arc::from("identity response exceeds its configured byte limit"),
            }
        })?;
    Ok(ProviderCall::completed(Some(response)))
}

pub(crate) fn success_response(
    id: &str,
    value: IdentityValue,
    limits: IdentityProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    let value = match value {
        IdentityValue::Relays(relays) => {
            json!({"type": "identity.getRelays.result", "id": id, "relays": relays})
        }
        IdentityValue::Profile(profile) => {
            json!({"type": "identity.getProfile.result", "id": id, "profile": profile})
        }
        IdentityValue::Follows(pubkeys) => {
            json!({"type": "identity.getFollows.result", "id": id, "pubkeys": pubkeys})
        }
        IdentityValue::List(entries) => {
            json!({"type": "identity.getList.result", "id": id, "entries": entries})
        }
        IdentityValue::Zaps(zaps) => {
            json!({"type": "identity.getZaps.result", "id": id, "zaps": zaps})
        }
        IdentityValue::Mutes(pubkeys) => {
            json!({"type": "identity.getMutes.result", "id": id, "pubkeys": pubkeys})
        }
        IdentityValue::Blocked(pubkeys) => {
            json!({"type": "identity.getBlocked.result", "id": id, "pubkeys": pubkeys})
        }
        IdentityValue::Badges(badges) => {
            json!({"type": "identity.getBadges.result", "id": id, "badges": badges})
        }
    };
    response(value, limits, request)
}

pub(crate) fn error_response(
    action: &str,
    id: &str,
    error: IdentityDataError,
    limits: IdentityProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    let error = error.to_string();
    let value = match action {
        "getRelays" => {
            json!({"type": "identity.getRelays.result", "id": id, "relays": {}, "error": error})
        }
        "getProfile" => {
            json!({"type": "identity.getProfile.result", "id": id, "profile": null, "error": error})
        }
        "getFollows" => {
            json!({"type": "identity.getFollows.result", "id": id, "pubkeys": [], "error": error})
        }
        "getList" => {
            json!({"type": "identity.getList.result", "id": id, "entries": [], "error": error})
        }
        "getZaps" => {
            json!({"type": "identity.getZaps.result", "id": id, "zaps": [], "error": error})
        }
        "getMutes" => {
            json!({"type": "identity.getMutes.result", "id": id, "pubkeys": [], "error": error})
        }
        "getBlocked" => {
            json!({"type": "identity.getBlocked.result", "id": id, "pubkeys": [], "error": error})
        }
        "getBadges" => {
            json!({"type": "identity.getBadges.result", "id": id, "badges": [], "error": error})
        }
        _ => return Err(invalid_payload(request, "unknown action")),
    };
    response(value, limits, request)
}

pub(crate) fn decode_identity_value(
    query: &IdentityQuery,
    value: &BoundedJson,
) -> Result<IdentityValue, ()> {
    let value = value.decode().map_err(|_| ())?;
    match query {
        IdentityQuery::Relays => serde_json::from_value(value)
            .map(IdentityValue::Relays)
            .map_err(|_| ()),
        IdentityQuery::Profile => serde_json::from_value(value)
            .map(IdentityValue::Profile)
            .map_err(|_| ()),
        IdentityQuery::Follows => serde_json::from_value(value)
            .map(IdentityValue::Follows)
            .map_err(|_| ()),
        IdentityQuery::List { .. } => serde_json::from_value(value)
            .map(IdentityValue::List)
            .map_err(|_| ()),
        IdentityQuery::Zaps => serde_json::from_value(value)
            .map(IdentityValue::Zaps)
            .map_err(|_| ()),
        IdentityQuery::Mutes => serde_json::from_value(value)
            .map(IdentityValue::Mutes)
            .map_err(|_| ()),
        IdentityQuery::Blocked => serde_json::from_value(value)
            .map(IdentityValue::Blocked)
            .map_err(|_| ()),
        IdentityQuery::Badges => serde_json::from_value(value)
            .map(IdentityValue::Badges)
            .map_err(|_| ()),
    }
}

#[cfg(test)]
pub(crate) fn encode_identity_value(value: &IdentityValue, maximum_bytes: usize) -> BoundedJson {
    let value = match value {
        IdentityValue::Relays(value) => serde_json::to_value(value),
        IdentityValue::Profile(value) => serde_json::to_value(value),
        IdentityValue::Follows(value)
        | IdentityValue::List(value)
        | IdentityValue::Mutes(value)
        | IdentityValue::Blocked(value) => serde_json::to_value(value),
        IdentityValue::Zaps(value) => serde_json::to_value(value),
        IdentityValue::Badges(value) => serde_json::to_value(value),
    }
    .expect("identity test value must serialize");
    BoundedJson::from_value(&value, maximum_bytes).expect("identity test value must fit")
}
