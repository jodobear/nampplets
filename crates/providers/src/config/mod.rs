use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushSender, ProviderRequest, ProviderSession, ProviderSessionContext,
    ProviderSessionEnd,
};
use nmp_native_runtime_core::{Capability, Principal, SessionId};
use nmp_native_runtime_store::RuntimeStore;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{PINNED_NAP_PROTOCOL, ProviderPushReport};

mod schema;
mod types;
mod value;
mod wire;

pub use types::{
    ConfigError, ConfigProviderLimits, ConfigSchemaErrorCode, SettingsExecutor,
    SettingsExecutorError, SettingsRequest,
};

use schema::validate_schema;
use value::{matching_targets, resolve_root, schema_sections, validate_value};
use wire::{
    bounded, correlation_id, exact_object, failed, invalid, lifecycle_error, response,
    schema_error, schema_error_envelope,
};

const CONFIG_DOMAIN: &str = "config";
const RECORD_KEY: &str = "record";
const MAX_SCHEMA_DEPTH: usize = 4;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ConfigRecord {
    schema: Value,
    version: Option<u64>,
    values: Value,
}

#[derive(Debug, Default)]
struct ConfigState {
    sessions: BTreeMap<SessionId, ConfigSession>,
}

#[derive(Clone, Debug)]
struct ConfigSession {
    principal: Principal,
    outbound: ProviderPushSender,
    subscribed: bool,
}

#[derive(Debug)]
pub struct ConfigProvider {
    store: Arc<RuntimeStore>,
    settings: Arc<dyn SettingsExecutor>,
    limits: ConfigProviderLimits,
    descriptor: ProviderDescriptor,
    state: Mutex<ConfigState>,
}

impl ConfigProvider {
    pub fn new(
        store: Arc<RuntimeStore>,
        settings: Arc<dyn SettingsExecutor>,
        limits: ConfigProviderLimits,
    ) -> Result<Self, ConfigError> {
        validate_limits(limits)?;
        Ok(Self {
            store,
            settings,
            limits,
            descriptor: ProviderDescriptor {
                domain: Capability::new(CONFIG_DOMAIN).expect("static capability is valid"),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)]),
                actions: [
                    "get",
                    "openSettings",
                    "registerSchema",
                    "subscribe",
                    "unsubscribe",
                ]
                .into_iter()
                .map(Arc::from)
                .collect(),
                sensitive: false,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            state: Mutex::new(ConfigState::default()),
        })
    }

    /// Installs a manifest-declared schema before untrusted code starts.
    pub fn register_manifest_schema(
        &self,
        principal: &Principal,
        schema: &Value,
        version: Option<u64>,
    ) -> Result<ProviderPushReport, ConfigError> {
        self.apply_schema(principal, schema, version)
    }

    /// Trusted shell-only value commit. No napplet wire action can call this.
    pub fn commit_values(
        &self,
        principal: &Principal,
        values: &Value,
    ) -> Result<ProviderPushReport, ConfigError> {
        let state = self.state.lock();
        let mut record = self.load_record(principal)?.ok_or(ConfigError::NoSchema)?;
        let object = values
            .as_object()
            .ok_or_else(|| ConfigError::InvalidValues(Arc::from("root must be an object")))?;
        validate_value(&record.schema, values, 0, self.limits)
            .map_err(ConfigError::InvalidValues)?;
        let resolved = resolve_root(&record.schema, Some(object), self.limits)?;
        validate_value(&record.schema, &resolved, 0, self.limits)
            .map_err(ConfigError::InvalidValues)?;
        record.values = resolved.clone();
        self.save_record(principal, &record)?;
        let targets = matching_targets(&state.sessions, principal);
        drop(state);
        self.push_values(targets, &resolved)
    }

    fn apply_schema(
        &self,
        principal: &Principal,
        schema: &Value,
        wire_version: Option<u64>,
    ) -> Result<ProviderPushReport, ConfigError> {
        validate_schema(schema, self.limits)?;
        let schema_version = schema
            .as_object()
            .and_then(|object| object.get("$version"))
            .and_then(Value::as_u64);
        if wire_version.is_some() && schema_version.is_some() && wire_version != schema_version {
            return Err(schema_error(
                ConfigSchemaErrorCode::VersionConflict,
                "wire version and schema `$version` differ",
            ));
        }
        let version = wire_version.or(schema_version);
        let state = self.state.lock();
        let current = self.load_record(principal)?;
        if let Some(current) = &current {
            if let (Some(old), Some(next)) = (current.version, version) {
                if next < old || (next == old && current.schema != *schema) {
                    return Err(schema_error(
                        ConfigSchemaErrorCode::VersionConflict,
                        "schema version does not advance the registered schema",
                    ));
                }
            }
        }
        let persisted = current
            .as_ref()
            .and_then(|record| record.values.as_object());
        let values = resolve_root(schema, persisted, self.limits)?;
        let record = ConfigRecord {
            schema: schema.clone(),
            version,
            values: values.clone(),
        };
        self.save_record(principal, &record)?;
        let targets = matching_targets(&state.sessions, principal);
        drop(state);
        self.push_values(targets, &values)
    }

    fn push_values(
        &self,
        targets: Vec<ProviderPushSender>,
        values: &Value,
    ) -> Result<ProviderPushReport, ConfigError> {
        bounded(
            &json!({"type":"config.values","values":values}),
            self.limits.maximum_response_bytes,
        )?;
        let mut report = ProviderPushReport::default();
        for outbound in targets {
            report.record(outbound.push(
                "config.values",
                Map::from_iter([("values".to_owned(), values.clone())]),
                Some("config.values"),
            ));
        }
        Ok(report)
    }

    fn register_schema_call(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        let payload = exact_object(&request, &["schema", "version"])?;
        let Some(schema) = payload.get("schema") else {
            return Err(invalid(&request, "`schema` is required"));
        };
        let version = match payload.get("version") {
            None => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .ok_or_else(|| invalid(&request, "`version` must be an unsigned integer"))?,
            ),
        };
        let value = match self.apply_schema(&request.principal, schema, version) {
            Ok(_) => json!({
                "type":"config.registerSchema.result",
                "id":id,
                "ok":true
            }),
            Err(ConfigError::Schema { code, message }) => json!({
                "type":"config.registerSchema.result",
                "id":id,
                "ok":false,
                "code":code.as_str(),
                "error":message
            }),
            Err(error) => return Err(failed(&request, error)),
        };
        response(value, self.limits, &request)
    }

    fn get(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        exact_object(&request, &[])?;
        let Some(record) = self
            .load_record(&request.principal)
            .map_err(|error| failed(&request, error))?
        else {
            return response(
                schema_error_envelope(ConfigSchemaErrorCode::NoSchema, "no schema is registered"),
                self.limits,
                &request,
            );
        };
        response(
            json!({"type":"config.values","id":id,"values":record.values}),
            self.limits,
            &request,
        )
    }

    fn subscribe(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        if request.correlation_id.is_some() {
            return Err(invalid(&request, "`config.subscribe` must not carry an id"));
        }
        exact_object(&request, &[])?;
        let Some(record) = self
            .load_record(&request.principal)
            .map_err(|error| failed(&request, error))?
        else {
            return response(
                schema_error_envelope(ConfigSchemaErrorCode::NoSchema, "no schema is registered"),
                self.limits,
                &request,
            );
        };
        let mut state = self.state.lock();
        if let Some(existing) = state.sessions.get_mut(&request.session) {
            if existing.principal != request.principal {
                return Err(ProviderError::Denied {
                    domain: Arc::from(CONFIG_DOMAIN),
                    action: Arc::clone(&request.action),
                    reason: Arc::from("mapped session identity changed"),
                });
            }
            existing.subscribed = true;
        } else {
            return Err(ProviderError::Denied {
                domain: Arc::from(CONFIG_DOMAIN),
                action: Arc::clone(&request.action),
                reason: Arc::from("config provider session is not mapped"),
            });
        }
        drop(state);
        response(
            json!({"type":"config.values","values":record.values}),
            self.limits,
            &request,
        )
    }

    fn unsubscribe(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        if request.correlation_id.is_some() {
            return Err(invalid(
                &request,
                "`config.unsubscribe` must not carry an id",
            ));
        }
        exact_object(&request, &[])?;
        let mut state = self.state.lock();
        let Some(session) = state.sessions.get_mut(&request.session) else {
            return Ok(ProviderCall::completed(None));
        };
        if session.principal != request.principal {
            return Err(ProviderError::Denied {
                domain: Arc::from(CONFIG_DOMAIN),
                action: Arc::clone(&request.action),
                reason: Arc::from("mapped session identity changed"),
            });
        }
        session.subscribed = false;
        Ok(ProviderCall::completed(None))
    }

    fn open_settings(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        if request.correlation_id.is_some() {
            return Err(invalid(
                &request,
                "`config.openSettings` must not carry an id",
            ));
        }
        let payload = exact_object(&request, &["section"])?;
        let requested_section: Option<Arc<str>> = match payload.get("section") {
            None => None,
            Some(Value::String(section)) => {
                if section.is_empty()
                    || section.len() > self.limits.maximum_section_bytes
                    || section
                        .bytes()
                        .any(|byte| byte == 0 || byte.is_ascii_control())
                {
                    return Err(invalid(&request, "`section` is invalid or too large"));
                }
                Some(Arc::from(section.as_str()))
            }
            Some(_) => return Err(invalid(&request, "`section` must be a string")),
        };
        let Some(record) = self
            .load_record(&request.principal)
            .map_err(|error| failed(&request, error))?
        else {
            return Ok(ProviderCall::completed(None));
        };
        let section = requested_section
            .filter(|section| schema_sections(&record.schema).contains(section.as_ref()));
        let schema = bounded(&record.schema, self.limits.maximum_schema_bytes)
            .map_err(|error| failed(&request, error))?;
        let values = bounded(&record.values, self.limits.maximum_values_bytes)
            .map_err(|error| failed(&request, error))?;
        self.settings
            .try_open(SettingsRequest {
                principal: request.principal.clone(),
                session: request.session,
                section,
                schema,
                values,
            })
            .map_err(|error| ProviderError::Failed {
                domain: Arc::from(CONFIG_DOMAIN),
                action: Arc::clone(&request.action),
                reason: Arc::from(error.to_string()),
            })?;
        Ok(ProviderCall::completed(None))
    }

    fn load_record(&self, principal: &Principal) -> Result<Option<ConfigRecord>, ConfigError> {
        self.store
            .component_value(principal, CONFIG_DOMAIN, RECORD_KEY)
            .map_err(|_| ConfigError::Persistence)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| ConfigError::Corrupt))
            .transpose()
    }

    fn save_record(&self, principal: &Principal, record: &ConfigRecord) -> Result<(), ConfigError> {
        let bytes = serde_json::to_vec(record).map_err(|_| ConfigError::Corrupt)?;
        if bytes.len() > self.limits.maximum_record_bytes {
            return Err(ConfigError::ResponseTooLarge);
        }
        self.store
            .put_component_value(principal, CONFIG_DOMAIN, RECORD_KEY, &bytes)
            .map_err(|_| ConfigError::Persistence)
    }
}

impl Provider for ConfigProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        match request.action.as_ref() {
            "registerSchema" => self.register_schema_call(request),
            "get" => self.get(request),
            "subscribe" => self.subscribe(request),
            "unsubscribe" => self.unsubscribe(request),
            "openSettings" => self.open_settings(request),
            _ => Err(invalid(&request, "unknown action")),
        }
    }

    fn session_opened(&self, session: ProviderSession) -> Result<(), ProviderError> {
        let mut state = self.state.lock();
        if let Some(existing) = state.sessions.get(&session.context.session) {
            return if existing.principal == session.context.principal
                && existing.outbound.source_window() == session.context.source_window
            {
                Ok(())
            } else {
                Err(lifecycle_error("mapped config session identity changed"))
            };
        }
        if state.sessions.len() >= self.limits.maximum_subscribed_sessions {
            return Err(lifecycle_error("config session capacity is full"));
        }
        state.sessions.insert(
            session.context.session,
            ConfigSession {
                principal: session.context.principal,
                outbound: session.outbound,
                subscribed: false,
            },
        );
        Ok(())
    }

    fn session_closed(&self, session: &ProviderSessionContext, _reason: ProviderSessionEnd) {
        self.remove_exact_session(session);
    }

    fn session_revoked(&self, session: &ProviderSessionContext) {
        self.remove_exact_session(session);
    }
}

impl ConfigProvider {
    fn remove_exact_session(&self, context: &ProviderSessionContext) {
        let mut state = self.state.lock();
        if state
            .sessions
            .get(&context.session)
            .is_some_and(|session| session.principal == context.principal)
        {
            state.sessions.remove(&context.session);
        }
    }
}

fn validate_limits(limits: ConfigProviderLimits) -> Result<(), ConfigError> {
    let fields = [
        limits.maximum_schema_bytes,
        limits.maximum_values_bytes,
        limits.maximum_record_bytes,
        limits.maximum_response_bytes,
        limits.maximum_correlation_id_bytes,
        limits.maximum_section_bytes,
        limits.maximum_subscribed_sessions,
        limits.maximum_properties_per_object,
        limits.maximum_array_items,
        limits.maximum_string_bytes,
        limits.maximum_enum_items,
    ];
    if fields.contains(&0)
        || limits.maximum_response_bytes < limits.maximum_values_bytes
        || limits.maximum_response_bytes < limits.maximum_schema_bytes
        || limits.maximum_record_bytes < limits.maximum_schema_bytes
        || limits.maximum_record_bytes < limits.maximum_values_bytes
    {
        return Err(ConfigError::InvalidLimits);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
