use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderRequest,
};
use nmp_native_runtime_core::{BoundedJson, Capability, SessionId};
use nmp_native_runtime_store::{RuntimeStore, StoreError};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use crate::PINNED_NAP_PROTOCOL;

const SHARED_SCOPE: &str = "storage.shared";
const INSTANCE_SCOPE_PREFIX: &str = "storage.instance.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageProviderLimits {
    pub maximum_key_bytes: usize,
    pub maximum_value_bytes: usize,
    pub maximum_keys_per_scope: usize,
    pub maximum_scope_bytes: usize,
    pub maximum_response_bytes: usize,
    pub maximum_correlation_id_bytes: usize,
}

impl Default for StorageProviderLimits {
    fn default() -> Self {
        Self {
            maximum_key_bytes: 256,
            maximum_value_bytes: 512 * 1024,
            maximum_keys_per_scope: 1_024,
            maximum_scope_bytes: 512 * 1024,
            maximum_response_bytes: 512 * 1024,
            maximum_correlation_id_bytes: 1_024,
        }
    }
}

#[derive(Debug)]
pub struct StorageProvider {
    store: Arc<RuntimeStore>,
    limits: StorageProviderLimits,
    descriptor: ProviderDescriptor,
    /// Serializes quota checks with writes at this provider-owned scope.
    mutations: Mutex<()>,
}

impl StorageProvider {
    pub fn new(
        store: Arc<RuntimeStore>,
        limits: StorageProviderLimits,
    ) -> Result<Self, ProviderError> {
        validate_limits(limits)?;
        Ok(Self {
            store,
            limits,
            descriptor: ProviderDescriptor {
                domain: Capability::new("storage").expect("static capability is valid"),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)]),
                actions: ["get", "set", "remove", "keys"]
                    .into_iter()
                    .map(Arc::from)
                    .collect(),
                sensitive: false,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            mutations: Mutex::new(()),
        })
    }

    fn get(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        let payload = exact_object(&request, &["key", "scope"])?;
        let key = required_string(payload, "key", &request)?;
        validate_key(key, self.limits, &request)?;
        let scope = storage_scope(payload, request.session, &request)?;

        let value = self
            .store
            .component_value(&request.principal, &scope, key)
            .map_err(|error| store_failure(&request, error))?
            .map(|bytes| {
                String::from_utf8(bytes).map_err(|_| ProviderError::Failed {
                    domain: Arc::from("storage"),
                    action: Arc::clone(&request.action),
                    reason: Arc::from("stored value is not valid UTF-8"),
                })
            })
            .transpose()?;
        response(
            json!({
                "type": "storage.get.result",
                "id": id,
                "value": value,
            }),
            self.limits,
            &request,
        )
    }

    fn set(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        let payload = exact_object(&request, &["key", "scope", "value"])?;
        let key = required_string(payload, "key", &request)?;
        validate_key(key, self.limits, &request)?;
        let value = required_string(payload, "value", &request)?;
        let scope = storage_scope(payload, request.session, &request)?;

        if value.len() > self.limits.maximum_value_bytes {
            return storage_error_response(
                "set",
                id,
                format!(
                    "value exceeds the {} byte limit",
                    self.limits.maximum_value_bytes
                ),
                self.limits,
                &request,
            );
        }

        let _guard = self.mutations.lock();
        let keys = match self.store.component_keys(
            &request.principal,
            &scope,
            self.limits.maximum_keys_per_scope,
        ) {
            Ok(keys) => keys,
            Err(error) => {
                return storage_error_response(
                    "set",
                    id,
                    public_store_error(&error),
                    self.limits,
                    &request,
                );
            }
        };
        let is_new = keys
            .binary_search_by(|candidate| candidate.as_str().cmp(key))
            .is_err();
        if is_new && keys.len() >= self.limits.maximum_keys_per_scope {
            return storage_error_response(
                "set",
                id,
                format!(
                    "storage key count exceeds the {} key limit",
                    self.limits.maximum_keys_per_scope
                ),
                self.limits,
                &request,
            );
        }

        let current = match self.store.component_value(&request.principal, &scope, key) {
            Ok(current) => current,
            Err(error) => {
                return storage_error_response(
                    "set",
                    id,
                    public_store_error(&error),
                    self.limits,
                    &request,
                );
            }
        };
        let mut used = 0usize;
        for stored_key in &keys {
            let Some(stored) = self
                .store
                .component_value(&request.principal, &scope, stored_key)
                .map_err(|error| store_failure(&request, error))?
            else {
                return Err(ProviderError::Failed {
                    domain: Arc::from("storage"),
                    action: Arc::clone(&request.action),
                    reason: Arc::from("storage scope changed during a serialized quota check"),
                });
            };
            used = used.saturating_add(stored.len());
        }
        let next = used
            .saturating_sub(current.as_ref().map_or(0, Vec::len))
            .saturating_add(value.len());
        if next > self.limits.maximum_scope_bytes {
            return storage_error_response(
                "set",
                id,
                format!(
                    "storage quota exceeds the {} byte limit",
                    self.limits.maximum_scope_bytes
                ),
                self.limits,
                &request,
            );
        }

        if let Err(error) =
            self.store
                .put_component_value(&request.principal, &scope, key, value.as_bytes())
        {
            return storage_error_response(
                "set",
                id,
                public_store_error(&error),
                self.limits,
                &request,
            );
        }
        response(
            json!({"type": "storage.set.result", "id": id}),
            self.limits,
            &request,
        )
    }

    fn remove(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        let payload = exact_object(&request, &["key", "scope"])?;
        let key = required_string(payload, "key", &request)?;
        validate_key(key, self.limits, &request)?;
        let scope = storage_scope(payload, request.session, &request)?;

        let _guard = self.mutations.lock();
        if let Err(error) = self
            .store
            .remove_component_value(&request.principal, &scope, key)
        {
            return storage_error_response(
                "remove",
                id,
                public_store_error(&error),
                self.limits,
                &request,
            );
        }
        response(
            json!({"type": "storage.remove.result", "id": id}),
            self.limits,
            &request,
        )
    }

    fn keys(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        let payload = exact_object(&request, &["scope"])?;
        let scope = storage_scope(payload, request.session, &request)?;
        let keys = match self.store.component_keys(
            &request.principal,
            &scope,
            self.limits.maximum_keys_per_scope,
        ) {
            Ok(keys) => keys,
            Err(error) => {
                return storage_error_response(
                    "keys",
                    id,
                    public_store_error(&error),
                    self.limits,
                    &request,
                );
            }
        };
        response(
            json!({"type": "storage.keys.result", "id": id, "keys": keys}),
            self.limits,
            &request,
        )
    }
}

impl Provider for StorageProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        match request.action.as_ref() {
            "get" => self.get(request),
            "set" => self.set(request),
            "remove" => self.remove(request),
            "keys" => self.keys(request),
            _ => Err(ProviderError::InvalidPayload {
                domain: Arc::from("storage"),
                action: Arc::clone(&request.action),
                reason: Arc::from("unknown action"),
            }),
        }
    }
}

fn validate_limits(limits: StorageProviderLimits) -> Result<(), ProviderError> {
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

fn correlation_id(
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

fn exact_object<'a>(
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

fn required_string<'a>(
    payload: &'a Map<String, Value>,
    field: &str,
    request: &ProviderRequest,
) -> Result<&'a str, ProviderError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(request, format!("`{field}` must be a string")))
}

fn validate_key(
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

fn storage_scope(
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

fn response(
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

fn storage_error_response(
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

fn public_store_error(error: &StoreError) -> String {
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

fn store_failure(request: &ProviderRequest, _error: StoreError) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from("storage"),
        action: Arc::clone(&request.action),
        reason: Arc::from("runtime storage operation failed"),
    }
}

fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from("storage"),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests;
