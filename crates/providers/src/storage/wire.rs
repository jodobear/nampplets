use std::sync::Arc;

use nmp_native_nap_bridge::{ProviderCall, ProviderError, ProviderRequest};
use nmp_native_runtime_core::{BoundedJson, SessionId};
use nmp_native_runtime_store::StoreError;
use serde_json::{Map, Value, json};

use super::{INSTANCE_SCOPE_PREFIX, SHARED_SCOPE, StorageProviderLimits};

pub(super) fn validate_limits(limits: StorageProviderLimits) -> Result<(), ProviderError> {
    if [
        limits.maximum_key_bytes,
        limits.maximum_value_bytes,
        limits.maximum_keys_per_scope,
        limits.maximum_scope_bytes,
        limits.maximum_response_bytes,
        limits.maximum_correlation_id_bytes,
    ]
    .contains(&0)
        || limits.maximum_value_bytes > limits.maximum_scope_bytes
    {
        return Err(ProviderError::Failed {
            domain: Arc::from("storage"),
            action: Arc::from("initialize"),
            reason: Arc::from(
                "provider limits must be finite, non-zero, and internally consistent",
            ),
        });
    }
    Ok(())
}

pub(super) fn correlation_id(
    request: &ProviderRequest,
    limits: StorageProviderLimits,
) -> Result<&str, ProviderError> {
    let id = request
        .correlation_id
        .as_deref()
        .ok_or_else(|| invalid(request, "id is required"))?;
    if id.len() > limits.maximum_correlation_id_bytes {
        return Err(invalid(
            request,
            format!(
                "id exceeds the {} byte limit",
                limits.maximum_correlation_id_bytes
            ),
        ));
    }
    Ok(id)
}

pub(super) fn exact_object<'a>(
    request: &'a ProviderRequest,
    allowed_fields: &[&str],
) -> Result<&'a Map<String, Value>, ProviderError> {
    let object = request
        .payload
        .as_object()
        .ok_or_else(|| invalid(request, "payload fields must form an object"))?;
    if let Some(field) = object
        .keys()
        .find(|field| !allowed_fields.contains(&field.as_str()))
    {
        return Err(invalid(request, format!("unexpected field `{field}`")));
    }
    Ok(object)
}

pub(super) fn required_string<'a>(
    payload: &'a Map<String, Value>,
    field: &str,
    request: &ProviderRequest,
) -> Result<&'a str, ProviderError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(request, format!("`{field}` must be a string")))
}

pub(super) fn validate_key(
    key: &str,
    limits: StorageProviderLimits,
    request: &ProviderRequest,
) -> Result<(), ProviderError> {
    if key.is_empty()
        || key.len() > limits.maximum_key_bytes
        || key.bytes().any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(invalid(
            request,
            format!(
                "`key` must be non-empty, control-free, and at most {} bytes",
                limits.maximum_key_bytes
            ),
        ));
    }
    Ok(())
}

pub(super) fn storage_scope(
    payload: &Map<String, Value>,
    session: SessionId,
    request: &ProviderRequest,
) -> Result<String, ProviderError> {
    match payload.get("scope") {
        None => Ok(SHARED_SCOPE.to_owned()),
        Some(Value::String(scope)) if scope == "shared" => Ok(SHARED_SCOPE.to_owned()),
        Some(Value::String(scope)) if scope == "instance" => {
            Ok(format!("{INSTANCE_SCOPE_PREFIX}{}", session.0))
        }
        _ => Err(invalid(
            request,
            "`scope` must be `shared`, `instance`, or omitted",
        )),
    }
}

pub(super) fn response(
    value: Value,
    limits: StorageProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    BoundedJson::from_value(&value, limits.maximum_response_bytes)
        .map(|response| ProviderCall::completed(Some(response)))
        .map_err(|_| ProviderError::Failed {
            domain: Arc::from("storage"),
            action: Arc::clone(&request.action),
            reason: Arc::from("response exceeds the configured byte limit"),
        })
}

pub(super) fn storage_error_response(
    action: &str,
    id: &str,
    error: String,
    limits: StorageProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    response(
        json!({
            "type": format!("storage.{action}.result"),
            "id": id,
            "error": error,
        }),
        limits,
        request,
    )
}

pub(super) fn public_store_error(error: &StoreError) -> String {
    match error {
        StoreError::KeyCapacity { capacity }
        | StoreError::KeyListCapacity {
            maximum: capacity, ..
        } => format!("storage key count exceeds the {capacity} key limit"),
        StoreError::ValueTooLarge { maximum, .. } => {
            format!("value exceeds the {maximum} byte limit")
        }
        StoreError::ScopeBytes { maximum, .. } => {
            format!("storage quota exceeds the {maximum} byte limit")
        }
        _ => "storage operation failed".to_owned(),
    }
}

pub(super) fn store_failure(request: &ProviderRequest, _error: StoreError) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from("storage"),
        action: Arc::clone(&request.action),
        reason: Arc::from("runtime storage operation failed"),
    }
}

pub(super) fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from("storage"),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}
