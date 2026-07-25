use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderRequest,
};
use nmp_native_runtime_core::Capability;
use nmp_native_runtime_store::RuntimeStore;
use parking_lot::Mutex;
use serde_json::json;

use crate::PINNED_NAP_PROTOCOL;

mod wire;

use wire::{
    correlation_id, exact_object, public_store_error, required_string, response,
    storage_error_response, storage_scope, store_failure, validate_key, validate_limits,
};

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

#[cfg(test)]
mod tests;
