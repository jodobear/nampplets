use std::sync::Arc;

use nmp_native_nap_bridge::{ProviderCall, ProviderError, ProviderRequest};
use nmp_native_runtime_core::BoundedJson;
use serde_json::{Map, Value, json};

use super::CONFIG_DOMAIN;
use super::types::{ConfigError, ConfigProviderLimits, ConfigSchemaErrorCode};

pub(super) fn exact_object<'a>(
    request: &'a ProviderRequest,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, ProviderError> {
    let object = request
        .payload
        .as_object()
        .ok_or_else(|| invalid(request, "payload fields must form an object"))?;
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(invalid(request, format!("unexpected field `{field}`")));
    }
    Ok(object)
}

pub(super) fn correlation_id(
    request: &ProviderRequest,
    limits: ConfigProviderLimits,
) -> Result<&str, ProviderError> {
    let id = request
        .correlation_id
        .as_deref()
        .ok_or_else(|| invalid(request, "`id` is required"))?;
    if id.is_empty()
        || id.len() > limits.maximum_correlation_id_bytes
        || id.bytes().any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(invalid(request, "`id` is invalid or too large"));
    }
    Ok(id)
}

pub(super) fn response(
    value: Value,
    limits: ConfigProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    bounded(&value, limits.maximum_response_bytes)
        .map(|response| ProviderCall::completed(Some(response)))
        .map_err(|error| failed(request, error))
}

pub(super) fn bounded(value: &Value, maximum: usize) -> Result<BoundedJson, ConfigError> {
    BoundedJson::from_value(value, maximum).map_err(|_| ConfigError::ResponseTooLarge)
}

pub(super) fn schema_error(
    code: ConfigSchemaErrorCode,
    message: impl Into<Arc<str>>,
) -> ConfigError {
    ConfigError::Schema {
        code,
        message: message.into(),
    }
}

pub(super) fn schema_error_envelope(code: ConfigSchemaErrorCode, message: &str) -> Value {
    json!({"type":"config.schemaError","code":code.as_str(),"error":message})
}

pub(super) fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(CONFIG_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

pub(super) fn failed(request: &ProviderRequest, error: ConfigError) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(CONFIG_DOMAIN),
        action: Arc::clone(&request.action),
        reason: Arc::from(error.to_string()),
    }
}

pub(super) fn lifecycle_error(reason: &'static str) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(CONFIG_DOMAIN),
        action: Arc::from("session"),
        reason: Arc::from(reason),
    }
}
