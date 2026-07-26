use std::{fmt, sync::Arc};

use nmp_native_runtime_core::{BoundedJson, Principal, SessionId};
use thiserror::Error;

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
