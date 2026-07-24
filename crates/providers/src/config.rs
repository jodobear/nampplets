use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushSender, ProviderRequest, ProviderSession, ProviderSessionContext,
    ProviderSessionEnd,
};
use nmp_native_runtime_core::{BoundedJson, Capability, Principal, SessionId};
use nmp_native_runtime_store::RuntimeStore;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::{PINNED_NAP_PROTOCOL, ProviderPushReport};

const CONFIG_DOMAIN: &str = "config";
const RECORD_KEY: &str = "record";
const MAX_SCHEMA_DEPTH: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigProviderLimits {
    pub maximum_schema_bytes: usize,
    pub maximum_values_bytes: usize,
    pub maximum_record_bytes: usize,
    pub maximum_response_bytes: usize,
    pub maximum_correlation_id_bytes: usize,
    pub maximum_section_bytes: usize,
    pub maximum_subscribed_sessions: usize,
    pub maximum_properties_per_object: usize,
    pub maximum_array_items: usize,
    pub maximum_string_bytes: usize,
    pub maximum_enum_items: usize,
}

impl Default for ConfigProviderLimits {
    fn default() -> Self {
        Self {
            maximum_schema_bytes: 192 * 1024,
            maximum_values_bytes: 192 * 1024,
            maximum_record_bytes: 512 * 1024,
            maximum_response_bytes: 512 * 1024,
            maximum_correlation_id_bytes: 1_024,
            maximum_section_bytes: 256,
            maximum_subscribed_sessions: 64,
            maximum_properties_per_object: 256,
            maximum_array_items: 1_024,
            maximum_string_bytes: 256 * 1024,
            maximum_enum_items: 256,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsRequest {
    pub principal: Principal,
    pub session: SessionId,
    pub section: Option<Arc<str>>,
    pub schema: BoundedJson,
    /// Validated and default-resolved values only. Implementations must treat
    /// this as secret-bearing and must not log or retain it after rendering.
    pub values: BoundedJson,
}

/// Bounded native capability executor. It executes presentation only; schema
/// policy, validation, persistence, and change semantics stay in Rust.
pub trait SettingsExecutor: Send + Sync + fmt::Debug {
    fn try_open(&self, request: SettingsRequest) -> Result<(), SettingsExecutorError>;
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SettingsExecutorError {
    #[error("native settings execution is saturated")]
    Saturated,
    #[error("native settings presentation is unavailable")]
    Unavailable,
    #[error("the mapped settings session is closed")]
    Closed,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("config provider limits must be finite, non-zero, and internally consistent")]
    InvalidLimits,
    #[error("config schema rejected with `{code}`: {message}")]
    Schema {
        code: ConfigSchemaErrorCode,
        message: Arc<str>,
    },
    #[error("config values do not validate: {0}")]
    InvalidValues(Arc<str>),
    #[error("config persistence failed")]
    Persistence,
    #[error("config state is corrupt")]
    Corrupt,
    #[error("config response exceeds the configured byte limit")]
    ResponseTooLarge,
    #[error("config subscription capacity is full at {maximum}")]
    SubscriptionCapacity { maximum: usize },
    #[error("the session is already bound to another exact-build principal")]
    SessionIdentityMismatch,
    #[error("no config schema is registered for this exact build")]
    NoSchema,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSchemaErrorCode {
    InvalidSchema,
    UnsupportedDraft,
    RefNotAllowed,
    PatternNotAllowed,
    SecretWithDefault,
    SchemaTooDeep,
    VersionConflict,
    NoSchema,
}

impl ConfigSchemaErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSchema => "invalid-schema",
            Self::UnsupportedDraft => "unsupported-draft",
            Self::RefNotAllowed => "ref-not-allowed",
            Self::PatternNotAllowed => "pattern-not-allowed",
            Self::SecretWithDefault => "secret-with-default",
            Self::SchemaTooDeep => "schema-too-deep",
            Self::VersionConflict => "version-conflict",
            Self::NoSchema => "no-schema",
        }
    }
}

impl fmt::Display for ConfigSchemaErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

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

fn validate_schema(schema: &Value, limits: ConfigProviderLimits) -> Result<(), ConfigError> {
    bounded(schema, limits.maximum_schema_bytes)?;
    let root = schema.as_object().ok_or_else(|| {
        schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "schema root must be an object",
        )
    })?;
    if root.get("type").and_then(Value::as_str) != Some("object")
        || !root.get("properties").is_some_and(Value::is_object)
    {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "schema root must declare object type and properties",
        ));
    }
    validate_schema_node(root, 1, true, limits)
}

fn validate_schema_node(
    schema: &Map<String, Value>,
    object_depth: usize,
    root: bool,
    limits: ConfigProviderLimits,
) -> Result<(), ConfigError> {
    for keyword in schema.keys() {
        if matches!(keyword.as_str(), "$ref" | "definitions" | "$defs") {
            return Err(schema_error(
                ConfigSchemaErrorCode::RefNotAllowed,
                format!("`{keyword}` is not allowed"),
            ));
        }
        if keyword == "pattern" {
            return Err(schema_error(
                ConfigSchemaErrorCode::PatternNotAllowed,
                "`pattern` is not allowed",
            ));
        }
        if matches!(
            keyword.as_str(),
            "oneOf"
                | "anyOf"
                | "allOf"
                | "not"
                | "if"
                | "then"
                | "else"
                | "patternProperties"
                | "propertyNames"
                | "dependencies"
                | "dependentSchemas"
                | "unevaluatedProperties"
                | "$dynamicRef"
        ) {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{keyword}` is outside the Core Subset"),
            ));
        }
        if !is_allowed_keyword(keyword, root) && !keyword.starts_with("x-napplet-") {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{keyword}` is outside the Core Subset"),
            ));
        }
    }

    if let Some(draft) = schema.get("$schema") {
        let Some(draft) = draft.as_str() else {
            return Err(schema_error(
                ConfigSchemaErrorCode::UnsupportedDraft,
                "`$schema` must be a supported draft URI",
            ));
        };
        if !matches!(
            draft,
            "http://json-schema.org/draft-07/schema#"
                | "https://json-schema.org/draft-07/schema"
                | "https://json-schema.org/draft/2019-09/schema"
                | "https://json-schema.org/draft/2020-12/schema"
        ) {
            return Err(schema_error(
                ConfigSchemaErrorCode::UnsupportedDraft,
                "only JSON Schema draft-07, 2019-09, and 2020-12 are supported",
            ));
        }
    }

    let schema_type = schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| schema_error(ConfigSchemaErrorCode::InvalidSchema, "`type` is required"))?;
    if !matches!(
        schema_type,
        "object" | "string" | "number" | "integer" | "boolean" | "array"
    ) {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "schema type is outside the Core Subset",
        ));
    }
    if root && schema_type != "object" {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "schema root type must be object",
        ));
    }

    for annotation in [
        "title",
        "description",
        "deprecationMessage",
        "markdownDescription",
        "format",
        "x-napplet-section",
    ] {
        if schema
            .get(annotation)
            .is_some_and(|value| !value.is_string())
        {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{annotation}` must be a string"),
            ));
        }
    }
    if schema
        .get("x-napplet-secret")
        .is_some_and(|value| !value.is_boolean())
        || schema.get("x-napplet-order").is_some_and(|value| {
            value
                .as_f64()
                .is_none_or(|number| !number.is_finite() || number < 0.0)
        })
    {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "invalid x-napplet extension value",
        ));
    }
    if schema.get("x-napplet-secret") == Some(&Value::Bool(true)) && schema.contains_key("default")
    {
        return Err(schema_error(
            ConfigSchemaErrorCode::SecretWithDefault,
            "secret properties cannot declare a default",
        ));
    }

    validate_numeric_constraints(schema)?;
    if let Some(enum_values) = schema.get("enum") {
        let Some(enum_values) = enum_values.as_array() else {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                "`enum` must be an array",
            ));
        };
        if enum_values.is_empty() || enum_values.len() > limits.maximum_enum_items {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                "`enum` has an invalid item count",
            ));
        }
        if let Some(descriptions) = schema.get("enumDescriptions") {
            let descriptions = descriptions.as_array().ok_or_else(|| {
                schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "`enumDescriptions` must be an array",
                )
            })?;
            if descriptions.len() != enum_values.len()
                || descriptions.iter().any(|value| !value.is_string())
            {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "`enumDescriptions` must parallel `enum` with strings",
                ));
            }
        }
    } else if schema.contains_key("enumDescriptions") {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "`enumDescriptions` requires `enum`",
        ));
    }

    match schema_type {
        "object" => {
            if object_depth > MAX_SCHEMA_DEPTH {
                return Err(schema_error(
                    ConfigSchemaErrorCode::SchemaTooDeep,
                    "object nesting exceeds 4 levels",
                ));
            }
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    schema_error(
                        ConfigSchemaErrorCode::InvalidSchema,
                        "object schemas must declare `properties`",
                    )
                })?;
            if properties.len() > limits.maximum_properties_per_object {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "object property count exceeds the configured limit",
                ));
            }
            if schema
                .get("additionalProperties")
                .is_some_and(|value| !value.is_boolean())
            {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "`additionalProperties` must be boolean",
                ));
            }
            if let Some(required) = schema.get("required") {
                let required = required.as_array().ok_or_else(|| {
                    schema_error(
                        ConfigSchemaErrorCode::InvalidSchema,
                        "`required` must be an array",
                    )
                })?;
                let mut unique = BTreeSet::new();
                for field in required {
                    let Some(field) = field.as_str() else {
                        return Err(schema_error(
                            ConfigSchemaErrorCode::InvalidSchema,
                            "`required` entries must be strings",
                        ));
                    };
                    if !properties.contains_key(field) || !unique.insert(field) {
                        return Err(schema_error(
                            ConfigSchemaErrorCode::InvalidSchema,
                            "`required` entries must be unique declared properties",
                        ));
                    }
                }
            }
            for property in properties.values() {
                let property = property.as_object().ok_or_else(|| {
                    schema_error(
                        ConfigSchemaErrorCode::InvalidSchema,
                        "property schemas must be objects",
                    )
                })?;
                let next_depth = if property.get("type").and_then(Value::as_str) == Some("object") {
                    object_depth.saturating_add(1)
                } else {
                    object_depth
                };
                validate_schema_node(property, next_depth, false, limits)?;
            }
        }
        "array" => {
            let items = schema
                .get("items")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    schema_error(
                        ConfigSchemaErrorCode::InvalidSchema,
                        "array `items` must be one homogeneous primitive schema",
                    )
                })?;
            if !matches!(
                items.get("type").and_then(Value::as_str),
                Some("string" | "number" | "integer" | "boolean")
            ) {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "Core Subset arrays may contain primitives only",
                ));
            }
            validate_schema_node(items, object_depth, false, limits)?;
        }
        _ => {
            if schema.contains_key("properties")
                || schema.contains_key("required")
                || schema.contains_key("items")
                || schema.contains_key("additionalProperties")
            {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "container keywords do not match the declared type",
                ));
            }
        }
    }
    if let Some(default) = schema.get("default") {
        validate_value(
            &Value::Object(schema.clone()),
            default,
            object_depth,
            limits,
        )
        .map_err(|reason| schema_error(ConfigSchemaErrorCode::InvalidSchema, reason))?;
    }
    Ok(())
}

fn is_allowed_keyword(keyword: &str, root: bool) -> bool {
    matches!(
        keyword,
        "type"
            | "properties"
            | "required"
            | "items"
            | "additionalProperties"
            | "default"
            | "title"
            | "description"
            | "enum"
            | "enumDescriptions"
            | "minimum"
            | "maximum"
            | "minLength"
            | "maxLength"
            | "minItems"
            | "maxItems"
            | "format"
            | "deprecationMessage"
            | "markdownDescription"
            | "$schema"
            | "$id"
            | "id"
    ) || (root && keyword == "$version")
}

fn validate_numeric_constraints(schema: &Map<String, Value>) -> Result<(), ConfigError> {
    for keyword in ["minimum", "maximum"] {
        if schema
            .get(keyword)
            .is_some_and(|value| value.as_f64().is_none_or(|number| !number.is_finite()))
        {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{keyword}` must be a finite number"),
            ));
        }
    }
    for keyword in ["minLength", "maxLength", "minItems", "maxItems", "$version"] {
        if schema
            .get(keyword)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{keyword}` must be an unsigned integer"),
            ));
        }
    }
    for (minimum, maximum) in [
        ("minimum", "maximum"),
        ("minLength", "maxLength"),
        ("minItems", "maxItems"),
    ] {
        if let (Some(minimum), Some(maximum)) = (schema.get(minimum), schema.get(maximum)) {
            let valid = match (minimum.as_f64(), maximum.as_f64()) {
                (Some(minimum), Some(maximum)) => minimum <= maximum,
                _ => false,
            };
            if !valid {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "minimum constraint exceeds maximum constraint",
                ));
            }
        }
    }
    Ok(())
}

fn validate_value(
    schema: &Value,
    value: &Value,
    depth: usize,
    limits: ConfigProviderLimits,
) -> Result<(), Arc<str>> {
    if depth > MAX_SCHEMA_DEPTH.saturating_add(1) {
        return Err(Arc::from("value nesting exceeds the schema depth limit"));
    }
    if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > limits.maximum_values_bytes) {
        return Err(Arc::from("value exceeds the configured byte limit"));
    }
    let schema = schema
        .as_object()
        .ok_or_else(|| Arc::from("schema node is not an object"))?;
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        if !enum_values.contains(value) {
            return Err(Arc::from("value is outside the declared enum"));
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => {
            let value = value
                .as_str()
                .ok_or_else(|| Arc::from("value must be a string"))?;
            if value.len() > limits.maximum_string_bytes {
                return Err(Arc::from("string exceeds the configured byte limit"));
            }
            validate_length(schema, value.chars().count(), "minLength", "maxLength")
        }
        Some("number") => {
            let number = value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| Arc::from("value must be a finite number"))?;
            validate_number(schema, number)
        }
        Some("integer") => {
            let number = value
                .as_i64()
                .map(|number| number as f64)
                .or_else(|| value.as_u64().map(|number| number as f64))
                .ok_or_else(|| Arc::from("value must be an integer"))?;
            validate_number(schema, number)
        }
        Some("boolean") => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(Arc::from("value must be a boolean"))
            }
        }
        Some("array") => {
            let values = value
                .as_array()
                .ok_or_else(|| Arc::from("value must be an array"))?;
            if values.len() > limits.maximum_array_items {
                return Err(Arc::from("array exceeds the configured item limit"));
            }
            validate_length(schema, values.len(), "minItems", "maxItems")?;
            let items = schema
                .get("items")
                .ok_or_else(|| Arc::from("array schema is missing items"))?;
            for value in values {
                validate_value(items, value, depth.saturating_add(1), limits)?;
            }
            Ok(())
        }
        Some("object") => {
            let object = value
                .as_object()
                .ok_or_else(|| Arc::from("value must be an object"))?;
            if object.len() > limits.maximum_properties_per_object {
                return Err(Arc::from("object exceeds the configured property limit"));
            }
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| Arc::from("object schema is missing properties"))?;
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for required in required {
                    if required
                        .as_str()
                        .is_some_and(|field| !object.contains_key(field))
                    {
                        return Err(Arc::from("required property is missing"));
                    }
                }
            }
            let allow_additional = schema
                .get("additionalProperties")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            for (field, value) in object {
                if let Some(property_schema) = properties.get(field) {
                    validate_value(property_schema, value, depth.saturating_add(1), limits)?;
                } else if !allow_additional {
                    return Err(Arc::from("object contains an undeclared property"));
                }
            }
            Ok(())
        }
        _ => Err(Arc::from("schema type is unsupported")),
    }
}

fn validate_length(
    schema: &Map<String, Value>,
    actual: usize,
    minimum: &str,
    maximum: &str,
) -> Result<(), Arc<str>> {
    if schema
        .get(minimum)
        .and_then(Value::as_u64)
        .is_some_and(|minimum| actual < usize::try_from(minimum).unwrap_or(usize::MAX))
    {
        return Err(Arc::from("value is shorter than the minimum"));
    }
    if schema
        .get(maximum)
        .and_then(Value::as_u64)
        .is_some_and(|maximum| actual > usize::try_from(maximum).unwrap_or(usize::MAX))
    {
        return Err(Arc::from("value exceeds the maximum length"));
    }
    Ok(())
}

fn validate_number(schema: &Map<String, Value>, number: f64) -> Result<(), Arc<str>> {
    if schema
        .get("minimum")
        .and_then(Value::as_f64)
        .is_some_and(|minimum| number < minimum)
    {
        return Err(Arc::from("number is below the minimum"));
    }
    if schema
        .get("maximum")
        .and_then(Value::as_f64)
        .is_some_and(|maximum| number > maximum)
    {
        return Err(Arc::from("number exceeds the maximum"));
    }
    Ok(())
}

fn resolve_root(
    schema: &Value,
    persisted: Option<&Map<String, Value>>,
    limits: ConfigProviderLimits,
) -> Result<Value, ConfigError> {
    let resolved = resolve_object(
        schema,
        persisted,
        schema.get("default").and_then(Value::as_object),
        0,
        limits,
    )?;
    bounded(&resolved, limits.maximum_values_bytes)?;
    Ok(resolved)
}

fn resolve_object(
    schema: &Value,
    persisted: Option<&Map<String, Value>>,
    ancestor_default: Option<&Map<String, Value>>,
    depth: usize,
    limits: ConfigProviderLimits,
) -> Result<Value, ConfigError> {
    let schema = schema
        .as_object()
        .ok_or_else(|| ConfigError::InvalidValues(Arc::from("schema node is invalid")))?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| ConfigError::InvalidValues(Arc::from("object schema has no properties")))?;
    let mut resolved = Map::new();
    for (field, property_schema) in properties {
        let candidate = persisted
            .and_then(|object| object.get(field))
            .filter(|value| validate_value(property_schema, value, depth, limits).is_ok())
            .or_else(|| property_schema.get("default"))
            .or_else(|| ancestor_default.and_then(|object| object.get(field)));
        let Some(candidate) = candidate else {
            continue;
        };
        let value = if property_schema.get("type").and_then(Value::as_str) == Some("object") {
            let Some(candidate) = candidate.as_object() else {
                continue;
            };
            resolve_object(
                property_schema,
                Some(candidate),
                property_schema.get("default").and_then(Value::as_object),
                depth.saturating_add(1),
                limits,
            )?
        } else {
            if validate_value(property_schema, candidate, depth, limits).is_err() {
                continue;
            }
            candidate.clone()
        };
        resolved.insert(field.clone(), value);
    }
    let value = Value::Object(resolved);
    validate_value(&Value::Object(schema.clone()), &value, depth, limits)
        .map_err(ConfigError::InvalidValues)?;
    Ok(value)
}

fn matching_targets(
    sessions: &BTreeMap<SessionId, ConfigSession>,
    principal: &Principal,
) -> Vec<ProviderPushSender> {
    sessions
        .values()
        .filter(|session| session.subscribed && &session.principal == principal)
        .map(|session| session.outbound.clone())
        .collect()
}

fn schema_sections(schema: &Value) -> BTreeSet<&str> {
    fn visit<'a>(schema: &'a Value, sections: &mut BTreeSet<&'a str>) {
        let Some(schema) = schema.as_object() else {
            return;
        };
        if let Some(section) = schema.get("x-napplet-section").and_then(Value::as_str) {
            sections.insert(section);
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for property in properties.values() {
                visit(property, sections);
            }
        }
    }
    let mut sections = BTreeSet::new();
    visit(schema, &mut sections);
    sections
}

fn exact_object<'a>(
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

fn correlation_id(
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

fn response(
    value: Value,
    limits: ConfigProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    bounded(&value, limits.maximum_response_bytes)
        .map(|response| ProviderCall::completed(Some(response)))
        .map_err(|error| failed(request, error))
}

fn bounded(value: &Value, maximum: usize) -> Result<BoundedJson, ConfigError> {
    BoundedJson::from_value(value, maximum).map_err(|_| ConfigError::ResponseTooLarge)
}

fn schema_error(code: ConfigSchemaErrorCode, message: impl Into<Arc<str>>) -> ConfigError {
    ConfigError::Schema {
        code,
        message: message.into(),
    }
}

fn schema_error_envelope(code: ConfigSchemaErrorCode, message: &str) -> Value {
    json!({"type":"config.schemaError","code":code.as_str(),"error":message})
}

fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(CONFIG_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn failed(request: &ProviderRequest, error: ConfigError) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(CONFIG_DOMAIN),
        action: Arc::clone(&request.action),
        reason: Arc::from(error.to_string()),
    }
}

fn lifecycle_error(reason: &'static str) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(CONFIG_DOMAIN),
        action: Arc::from("session"),
        reason: Arc::from(reason),
    }
}

#[cfg(test)]
mod tests {
    use nmp_native_nap_bridge::{
        ActivitySink, BridgeLimits, ProviderActivity, ProviderPushObserver, ProviderRegistry,
        SessionContext, SourceWindowId,
    };
    use nmp_native_runtime_core::{
        ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, ResourceClass, ResourceLimits,
        ResourceTracker, Sensitivity,
    };
    use nmp_native_runtime_store::StoreLimits;
    use parking_lot::Mutex;
    use tempfile::TempDir;

    use super::*;
    #[derive(Debug, Default)]
    struct Settings(Mutex<Vec<SettingsRequest>>);

    impl SettingsExecutor for Settings {
        fn try_open(&self, request: SettingsRequest) -> Result<(), SettingsExecutorError> {
            self.0.lock().push(request);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct NoActivity;

    impl ActivitySink for NoActivity {
        fn record(&self, _fact: ProviderActivity) {}
    }

    struct Rig {
        _directory: TempDir,
        provider: Arc<ConfigProvider>,
        registry: ProviderRegistry,
        grants: Arc<GrantLedger>,
        settings: Arc<Settings>,
        resources: Arc<ResourceTracker>,
    }

    impl Rig {
        fn new(limits: ConfigProviderLimits) -> Self {
            let directory = TempDir::new().unwrap();
            let store = Arc::new(
                RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default())
                    .unwrap(),
            );
            let settings = Arc::new(Settings::default());
            let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
            let grants =
                Arc::new(GrantLedger::new(GrantLimits::default(), resources.clone()).unwrap());
            let provider = Arc::new(ConfigProvider::new(store, settings.clone(), limits).unwrap());
            let mut registry = ProviderRegistry::new(
                BridgeLimits::default(),
                resources.clone(),
                grants.clone(),
                Arc::new(NoActivity),
            )
            .unwrap();
            registry.register(provider.clone()).unwrap();
            Self {
                _directory: directory,
                provider,
                registry,
                grants,
                settings,
                resources,
            }
        }

        fn open(
            &self,
            principal: &Principal,
            session: u64,
        ) -> Result<ProviderPushObserver, nmp_native_nap_bridge::BridgeError> {
            let capability = Capability::new(CONFIG_DOMAIN).unwrap();
            self.grants
                .set(
                    principal.clone(),
                    capability.clone(),
                    Sensitivity::Ordinary,
                    GrantDecision::AllowExactBuild,
                )
                .unwrap();
            let context = SessionContext {
                id: SessionId(session),
                principal: principal.clone(),
                profile: ExecutionProfile::Legacy,
            };
            let plan = self.registry.negotiate(
                principal,
                ExecutionProfile::Legacy,
                &BTreeSet::from([capability]),
            )?;
            self.registry.open_session_bound(
                &context,
                &plan,
                SourceWindowId(session.saturating_add(1_000)),
                0,
            )
        }

        fn call(
            &self,
            principal: Principal,
            session: u64,
            action: &str,
            id: Option<&str>,
            payload: Value,
        ) -> Result<Option<Value>, ProviderError> {
            let call = self.provider.call(ProviderRequest {
                principal,
                session: SessionId(session),
                action: Arc::from(action),
                correlation_id: id.map(Arc::from),
                payload,
                work: self
                    .resources
                    .admit(
                        SessionId(session),
                        Some(Capability::new(CONFIG_DOMAIN).unwrap()),
                        ResourceClass::ProviderCall,
                    )
                    .unwrap(),
            })?;
            Ok(call
                .response
                .map(|response| response.decode().expect("valid response")))
        }
    }

    fn principal(hash: char) -> Principal {
        Principal::new("a".repeat(64), "config-app", hash.to_string().repeat(64)).unwrap()
    }

    fn schema() -> Value {
        json!({
            "$schema":"http://json-schema.org/draft-07/schema#",
            "type":"object",
            "properties":{
                "theme":{
                    "type":"string",
                    "enum":["light","dark"],
                    "default":"dark",
                    "x-napplet-section":"appearance"
                },
                "size":{"type":"integer","minimum":10,"maximum":20,"default":12},
                "nested":{
                    "type":"object",
                    "properties":{"enabled":{"type":"boolean"}},
                    "default":{"enabled":true}
                },
                "token":{"type":"string","x-napplet-secret":true}
            },
            "additionalProperties":false
        })
    }

    #[test]
    fn descriptor_covers_every_pinned_outbound_action() {
        let rig = Rig::new(ConfigProviderLimits::default());
        assert_eq!(
            rig.provider.descriptor.actions,
            [
                "get",
                "openSettings",
                "registerSchema",
                "subscribe",
                "unsubscribe"
            ]
            .into_iter()
            .map(Arc::from)
            .collect()
        );
        assert_eq!(
            rig.provider.descriptor.protocol_versions,
            BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)])
        );
    }

    #[test]
    fn schema_register_get_defaults_and_exact_build_isolation() {
        let rig = Rig::new(ConfigProviderLimits::default());
        let owner = principal('b');
        rig.open(&owner, 1).unwrap();
        assert_eq!(
            rig.call(
                owner.clone(),
                1,
                "registerSchema",
                Some("r1"),
                json!({"schema":schema(),"version":1})
            )
            .unwrap()
            .unwrap(),
            json!({"type":"config.registerSchema.result","id":"r1","ok":true})
        );
        assert_eq!(
            rig.call(owner.clone(), 1, "get", Some("g1"), json!({}))
                .unwrap()
                .unwrap(),
            json!({
                "type":"config.values",
                "id":"g1",
                "values":{"theme":"dark","size":12,"nested":{"enabled":true}}
            })
        );
        let update = principal('c');
        rig.open(&update, 2).unwrap();
        assert_eq!(
            rig.call(update, 2, "get", Some("g2"), json!({}))
                .unwrap()
                .unwrap(),
            json!({
                "type":"config.schemaError",
                "code":"no-schema",
                "error":"no schema is registered"
            })
        );
    }

    #[test]
    fn forbidden_schema_features_return_correlated_negative_ack() {
        let rig = Rig::new(ConfigProviderLimits::default());
        let owner = principal('b');
        rig.open(&owner, 1).unwrap();
        for (schema, code) in [
            (
                json!({"type":"object","properties":{"x":{"type":"string","pattern":"x"}}}),
                "pattern-not-allowed",
            ),
            (
                json!({"type":"object","properties":{"x":{"type":"string","$ref":"#"}}}),
                "ref-not-allowed",
            ),
            (
                json!({"type":"object","properties":{"x":{"type":"string","x-napplet-secret":true,"default":"bad"}}}),
                "secret-with-default",
            ),
        ] {
            let response = rig
                .call(
                    owner.clone(),
                    1,
                    "registerSchema",
                    Some("bad"),
                    json!({"schema":schema}),
                )
                .unwrap()
                .unwrap();
            assert_eq!(response["ok"], false);
            assert_eq!(response["code"], code);
        }
    }

    #[test]
    fn subscribe_push_commit_and_teardown_are_exact_and_bounded() {
        let limits = ConfigProviderLimits {
            maximum_subscribed_sessions: 1,
            ..ConfigProviderLimits::default()
        };
        let rig = Rig::new(limits);
        let owner = principal('b');
        let observer = rig.open(&owner, 1).unwrap();
        rig.provider
            .register_manifest_schema(&owner, &schema(), Some(1))
            .unwrap();
        assert_eq!(
            rig.call(owner.clone(), 1, "subscribe", None, json!({}))
                .unwrap()
                .unwrap(),
            json!({
                "type":"config.values",
                "values":{"theme":"dark","size":12,"nested":{"enabled":true}}
            })
        );
        assert!(rig.open(&owner, 2).is_err());
        let report = rig
            .provider
            .commit_values(
                &owner,
                &json!({"theme":"light","size":14,"nested":{"enabled":false},"token":"secret"}),
            )
            .unwrap();
        assert_eq!(
            report,
            ProviderPushReport {
                attempted: 1,
                delivered: 1,
                refused: 0
            }
        );
        let push = observer.drain(8).unwrap().pushes.pop().unwrap();
        assert_eq!(push.session, SessionId(1));
        assert_eq!(
            push.envelope.decode().unwrap(),
            json!({
                "type":"config.values",
                "values":{"theme":"light","size":14,"nested":{"enabled":false},"token":"secret"}
            })
        );
        rig.registry.close_session(SessionId(1));
        assert_eq!(
            rig.provider
                .commit_values(
                    &owner,
                    &json!({"theme":"dark","size":13,"nested":{"enabled":true}})
                )
                .unwrap()
                .attempted,
            0
        );
    }

    #[test]
    fn schema_change_drops_orphans_and_secret_values_before_delivery() {
        let rig = Rig::new(ConfigProviderLimits::default());
        let owner = principal('b');
        rig.open(&owner, 1).unwrap();
        rig.provider
            .register_manifest_schema(&owner, &schema(), Some(1))
            .unwrap();
        rig.provider
            .commit_values(
                &owner,
                &json!({"theme":"light","size":14,"nested":{"enabled":false},"token":"secret"}),
            )
            .unwrap();
        let next = json!({
            "type":"object",
            "properties":{"theme":{"type":"string","default":"dark"}},
            "additionalProperties":false
        });
        rig.provider
            .register_manifest_schema(&owner, &next, Some(2))
            .unwrap();
        assert_eq!(
            rig.call(owner, 1, "get", Some("g"), json!({}))
                .unwrap()
                .unwrap()["values"],
            json!({"theme":"light"})
        );
    }

    #[test]
    fn settings_executor_receives_bounded_validated_data_not_a_native_handle() {
        let rig = Rig::new(ConfigProviderLimits::default());
        let owner = principal('b');
        rig.open(&owner, 9).unwrap();
        rig.provider
            .register_manifest_schema(&owner, &schema(), Some(1))
            .unwrap();
        assert!(
            rig.call(
                owner.clone(),
                9,
                "openSettings",
                None,
                json!({"section":"appearance"})
            )
            .unwrap()
            .is_none()
        );
        let request = rig.settings.0.lock().pop().unwrap();
        assert_eq!(request.principal, owner);
        assert_eq!(request.session, SessionId(9));
        assert_eq!(request.section.as_deref(), Some("appearance"));
        assert!(request.schema.byte_len() > 0);
        assert!(request.values.byte_len() > 0);
    }
}
