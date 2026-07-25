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
use parking_lot::Mutex;
use serde_json::{Value, json};
use thiserror::Error;

use crate::{PINNED_NAP_PROTOCOL, ProviderPushReport};

const THEME_DOMAIN: &str = "theme";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeProviderLimits {
    pub maximum_theme_bytes: usize,
    pub maximum_response_bytes: usize,
    pub maximum_correlation_id_bytes: usize,
    pub maximum_string_bytes: usize,
    pub maximum_declaring_ready_sessions: usize,
}

impl Default for ThemeProviderLimits {
    fn default() -> Self {
        Self {
            maximum_theme_bytes: 64 * 1024,
            maximum_response_bytes: 128 * 1024,
            maximum_correlation_id_bytes: 1_024,
            maximum_string_bytes: 8 * 1024,
            maximum_declaring_ready_sessions: 64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeSnapshot {
    value: BoundedJson,
}

impl ThemeSnapshot {
    pub fn from_value(value: &Value, limits: ThemeProviderLimits) -> Result<Self, ThemeError> {
        validate_theme(value, limits)?;
        let value = BoundedJson::from_value(value, limits.maximum_theme_bytes).map_err(|_| {
            ThemeError::TooLarge {
                maximum_bytes: limits.maximum_theme_bytes,
            }
        })?;
        Ok(Self { value })
    }

    pub fn value(&self) -> &BoundedJson {
        &self.value
    }
}

pub trait ThemeSource: Send + Sync + fmt::Debug {
    /// Returns the current host-owned theme; the provider never persists or
    /// derives theme policy.
    fn current(&self) -> Option<ThemeSnapshot>;
}

#[derive(Debug)]
pub struct ThemeProvider {
    source: Arc<dyn ThemeSource>,
    limits: ThemeProviderLimits,
    sessions: Mutex<BTreeMap<SessionId, ThemeSession>>,
    descriptor: ProviderDescriptor,
}

#[derive(Clone, Debug)]
struct ThemeSession {
    principal: Principal,
    outbound: ProviderPushSender,
    ready: bool,
}

impl ThemeProvider {
    pub fn new(
        source: Arc<dyn ThemeSource>,
        limits: ThemeProviderLimits,
    ) -> Result<Self, ThemeError> {
        validate_limits(limits)?;
        Ok(Self {
            source,
            limits,
            sessions: Mutex::new(BTreeMap::new()),
            descriptor: ProviderDescriptor {
                domain: Capability::new(THEME_DOMAIN).expect("static capability is valid"),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)]),
                actions: BTreeSet::from([Arc::from("get")]),
                sensitive: false,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
        })
    }

    /// Delivers the latest host-owned theme to every declaring ready session.
    /// The push sink is finite and conflating; any refusal is visible in the
    /// returned report rather than silently truncated.
    pub fn publish_changed(
        &self,
        snapshot: &ThemeSnapshot,
    ) -> Result<ProviderPushReport, ThemeError> {
        let theme = snapshot
            .value()
            .decode()
            .map_err(|_| ThemeError::InvalidField("encoded theme"))?;
        validate_theme(&theme, self.limits)?;
        BoundedJson::from_value(
            &json!({"type":"theme.changed","theme":theme}),
            self.limits.maximum_response_bytes,
        )
        .map_err(|_| ThemeError::TooLarge {
            maximum_bytes: self.limits.maximum_response_bytes,
        })?;
        let targets = self
            .sessions
            .lock()
            .values()
            .filter(|session| session.ready)
            .map(|session| session.outbound.clone())
            .collect::<Vec<_>>();
        let mut report = ProviderPushReport::default();
        for outbound in targets {
            report.record(outbound.push(
                "theme.changed",
                serde_json::Map::from_iter([("theme".to_owned(), theme.clone())]),
                Some("theme.changed"),
            ));
        }
        Ok(report)
    }

    fn get(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = request
            .correlation_id
            .as_deref()
            .ok_or_else(|| invalid(&request, "`id` is required"))?;
        if id.is_empty()
            || id.len() > self.limits.maximum_correlation_id_bytes
            || id.bytes().any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(invalid(&request, "`id` is invalid or too large"));
        }
        if request
            .payload
            .as_object()
            .is_none_or(|payload| !payload.is_empty())
        {
            return Err(invalid(
                &request,
                "`theme.get` must carry no payload fields",
            ));
        }
        let value = self.source.current().map_or_else(
            || {
                json!({
                    "type": "theme.get.result",
                    "id": id,
                    "error": "no active theme",
                })
            },
            |snapshot| {
                json!({
                    "type": "theme.get.result",
                    "id": id,
                    "theme": snapshot.value().decode().expect("validated theme JSON"),
                })
            },
        );
        BoundedJson::from_value(&value, self.limits.maximum_response_bytes)
            .map(|response| ProviderCall::completed(Some(response)))
            .map_err(|_| ProviderError::Failed {
                domain: Arc::from(THEME_DOMAIN),
                action: Arc::clone(&request.action),
                reason: Arc::from("response exceeds the configured byte limit"),
            })
    }
}

impl Provider for ThemeProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        if request.action.as_ref() == "get" {
            self.get(request)
        } else {
            Err(invalid(&request, "unknown action"))
        }
    }

    fn session_opened(&self, session: ProviderSession) -> Result<(), ProviderError> {
        let mut sessions = self.sessions.lock();
        if let Some(existing) = sessions.get(&session.context.session) {
            return if existing.principal == session.context.principal
                && existing.outbound.source_window() == session.context.source_window
            {
                Ok(())
            } else {
                Err(lifecycle_error("mapped theme session identity changed"))
            };
        }
        if sessions.len() >= self.limits.maximum_declaring_ready_sessions {
            return Err(lifecycle_error("theme declaring-session capacity is full"));
        }
        sessions.insert(
            session.context.session,
            ThemeSession {
                principal: session.context.principal,
                outbound: session.outbound,
                ready: false,
            },
        );
        Ok(())
    }

    fn session_ready(&self, session: &ProviderSessionContext) -> Result<(), ProviderError> {
        let mut sessions = self.sessions.lock();
        let Some(existing) = sessions.get_mut(&session.session) else {
            return Err(lifecycle_error("theme session was not opened"));
        };
        if existing.principal != session.principal
            || existing.outbound.source_window() != session.source_window
        {
            return Err(lifecycle_error("mapped theme session identity changed"));
        }
        existing.ready = true;
        Ok(())
    }

    fn session_closed(&self, session: &ProviderSessionContext, _reason: ProviderSessionEnd) {
        self.remove_exact_session(session);
    }

    fn session_revoked(&self, session: &ProviderSessionContext) {
        self.remove_exact_session(session);
    }
}

impl ThemeProvider {
    fn remove_exact_session(&self, context: &ProviderSessionContext) {
        let mut sessions = self.sessions.lock();
        if sessions
            .get(&context.session)
            .is_some_and(|session| session.principal == context.principal)
        {
            sessions.remove(&context.session);
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ThemeError {
    #[error("theme limits must be finite, non-zero, and internally consistent")]
    InvalidLimit,
    #[error("theme payload exceeds {maximum_bytes} bytes")]
    TooLarge { maximum_bytes: usize },
    #[error(
        "theme payload must be an object with colors.background, colors.text, and colors.primary strings"
    )]
    InvalidColors,
    #[error("theme field `{0}` has an invalid shape")]
    InvalidField(&'static str),
    #[error("theme declaring-session capacity is full at {maximum}")]
    SessionCapacity { maximum: usize },
    #[error("the mapped theme session identity changed")]
    SessionIdentityMismatch,
}

fn validate_limits(limits: ThemeProviderLimits) -> Result<(), ThemeError> {
    if [
        limits.maximum_theme_bytes,
        limits.maximum_response_bytes,
        limits.maximum_correlation_id_bytes,
        limits.maximum_string_bytes,
        limits.maximum_declaring_ready_sessions,
    ]
    .contains(&0)
        || limits.maximum_response_bytes < limits.maximum_theme_bytes
    {
        return Err(ThemeError::InvalidLimit);
    }
    Ok(())
}

fn validate_theme(value: &Value, limits: ThemeProviderLimits) -> Result<(), ThemeError> {
    let object = value.as_object().ok_or(ThemeError::InvalidColors)?;
    let colors = object
        .get("colors")
        .and_then(Value::as_object)
        .ok_or(ThemeError::InvalidColors)?;
    for field in ["background", "text", "primary"] {
        if colors
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(|value| value.len() > limits.maximum_string_bytes)
        {
            return Err(ThemeError::InvalidColors);
        }
    }
    if colors.len() != 3 {
        return Err(ThemeError::InvalidColors);
    }
    if let Some(fonts) = object.get("fonts") {
        let fonts = fonts.as_object().ok_or(ThemeError::InvalidField("fonts"))?;
        if fonts
            .keys()
            .any(|field| !matches!(field.as_str(), "body" | "title"))
        {
            return Err(ThemeError::InvalidField("fonts"));
        }
        for font in fonts.values() {
            let font = font.as_object().ok_or(ThemeError::InvalidField("fonts"))?;
            if font.len() != 2
                || ["name", "url"].into_iter().any(|field| {
                    font.get(field)
                        .and_then(Value::as_str)
                        .is_none_or(|value| value.len() > limits.maximum_string_bytes)
                })
            {
                return Err(ThemeError::InvalidField("fonts"));
            }
        }
    }
    if let Some(background) = object.get("background") {
        let background = background
            .as_object()
            .ok_or(ThemeError::InvalidField("background"))?;
        if background.len() != 3
            || ["url", "mode", "mime"].into_iter().any(|field| {
                background
                    .get(field)
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.len() > limits.maximum_string_bytes)
            })
        {
            return Err(ThemeError::InvalidField("background"));
        }
    }
    if object.get("title").is_some_and(|title| {
        title
            .as_str()
            .is_none_or(|value| value.len() > limits.maximum_string_bytes)
    }) {
        return Err(ThemeError::InvalidField("title"));
    }
    if object
        .keys()
        .any(|field| !matches!(field.as_str(), "colors" | "fonts" | "background" | "title"))
    {
        return Err(ThemeError::InvalidField("unknown"));
    }
    BoundedJson::from_value(value, limits.maximum_theme_bytes)
        .map(|_| ())
        .map_err(|_| ThemeError::TooLarge {
            maximum_bytes: limits.maximum_theme_bytes,
        })
}

fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(THEME_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn lifecycle_error(reason: &'static str) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(THEME_DOMAIN),
        action: Arc::from("session"),
        reason: Arc::from(reason),
    }
}

#[cfg(test)]
mod tests;
