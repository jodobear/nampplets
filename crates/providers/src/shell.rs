use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderRequest,
};
use nmp_native_runtime_core::{BoundedJson, Capability, Principal, SessionId};
use parking_lot::Mutex;
use serde_json::json;
use thiserror::Error;

use crate::PINNED_SHELL_PROTOCOL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellProviderLimits {
    pub maximum_ready_sessions: usize,
}

impl Default for ShellProviderLimits {
    fn default() -> Self {
        Self {
            maximum_ready_sessions: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellEnvironmentLimits {
    pub maximum_domains: usize,
    pub maximum_services: usize,
    pub maximum_service_name_bytes: usize,
    pub maximum_response_bytes: usize,
}

impl Default for ShellEnvironmentLimits {
    fn default() -> Self {
        Self {
            maximum_domains: 64,
            maximum_services: 64,
            maximum_service_name_bytes: 256,
            maximum_response_bytes: 64 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellEnvironment {
    domains: BTreeSet<Capability>,
    init: BoundedJson,
}

impl ShellEnvironment {
    pub fn new(
        domains: impl IntoIterator<Item = Capability>,
        services: impl IntoIterator<Item = Arc<str>>,
        limits: ShellEnvironmentLimits,
    ) -> Result<Self, ShellEnvironmentError> {
        validate_environment_limits(limits)?;
        let domains = domains.into_iter().collect::<BTreeSet<_>>();
        if domains.len() > limits.maximum_domains {
            return Err(ShellEnvironmentError::DomainCapacity {
                actual: domains.len(),
                maximum: limits.maximum_domains,
            });
        }
        let services = services.into_iter().collect::<BTreeSet<_>>();
        if services.len() > limits.maximum_services {
            return Err(ShellEnvironmentError::ServiceCapacity {
                actual: services.len(),
                maximum: limits.maximum_services,
            });
        }
        for service in &services {
            if service.is_empty()
                || service.len() > limits.maximum_service_name_bytes
                || service
                    .bytes()
                    .any(|byte| byte == 0 || byte.is_ascii_control())
            {
                return Err(ShellEnvironmentError::InvalidServiceName);
            }
        }
        let init = BoundedJson::from_value(
            &json!({
                "type": "shell.init",
                "capabilities": {
                    "domains": domains
                        .iter()
                        .map(|domain| domain.to_string())
                        .collect::<Vec<_>>()
                },
                "services": services,
            }),
            limits.maximum_response_bytes,
        )
        .map_err(|_| ShellEnvironmentError::ResponseTooLarge {
            maximum: limits.maximum_response_bytes,
        })?;
        Ok(Self { domains, init })
    }

    fn init(&self) -> BoundedJson {
        self.init.clone()
    }

    fn domains(&self) -> &BTreeSet<Capability> {
        &self.domains
    }
}

pub trait ShellEnvironmentSource: Send + Sync + fmt::Debug {
    /// Produces the environment for this mapped exact-build session from the
    /// kernel-owned, immutable negotiated domain set.
    fn environment(
        &self,
        principal: &Principal,
        session: SessionId,
        offered_domains: &BTreeSet<Capability>,
    ) -> Result<ShellEnvironment, ShellEnvironmentError>;
}

/// Bounded owner of the mandatory NAP-SHELL readiness handshake.
///
/// `shell.init` is an uncorrelated fire-and-forget response, not a synthetic
/// `shell.ready.result`. Runtime lifecycle code must gate every non-shell call
/// on [`ShellProvider::is_ready`] and call [`ShellProvider::close_session`] at
/// teardown before claiming the complete handshake contract.
#[derive(Debug)]
pub struct ShellProvider {
    source: Arc<dyn ShellEnvironmentSource>,
    limits: ShellProviderLimits,
    sessions: Mutex<BTreeMap<SessionId, PreparedSession>>,
    descriptor: ProviderDescriptor,
}

#[derive(Debug)]
struct PreparedSession {
    principal: Principal,
    environment: ShellEnvironment,
    ready: bool,
}

impl ShellProvider {
    pub fn new(
        source: Arc<dyn ShellEnvironmentSource>,
        limits: ShellProviderLimits,
    ) -> Result<Self, ProviderError> {
        if limits.maximum_ready_sessions == 0 {
            return Err(ProviderError::Failed {
                domain: Arc::from("shell"),
                action: Arc::from("initialize"),
                reason: Arc::from("ready session capacity must be finite and non-zero"),
            });
        }
        Ok(Self {
            source,
            limits,
            sessions: Mutex::new(BTreeMap::new()),
            descriptor: ProviderDescriptor {
                domain: Capability::new("shell").expect("static capability is valid"),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_SHELL_PROTOCOL)]),
                actions: BTreeSet::from([Arc::from("ready")]),
                sensitive: false,
            },
        })
    }

    /// Freezes the exact kernel-negotiated capability set before untrusted
    /// content can signal readiness.
    pub fn prepare_session(
        &self,
        principal: &Principal,
        session: SessionId,
        offered_domains: &BTreeSet<Capability>,
    ) -> Result<(), ShellEnvironmentError> {
        if !offered_domains
            .iter()
            .any(|domain| domain.as_str() == "shell")
        {
            return Err(ShellEnvironmentError::MissingFoundationalShell);
        }
        {
            let sessions = self.sessions.lock();
            if let Some(existing) = sessions.get(&session) {
                return if &existing.principal == principal
                    && existing.environment.domains() == offered_domains
                {
                    Ok(())
                } else {
                    Err(ShellEnvironmentError::SessionIdentityMismatch)
                };
            }
            if sessions.len() >= self.limits.maximum_ready_sessions {
                return Err(ShellEnvironmentError::SessionCapacity {
                    maximum: self.limits.maximum_ready_sessions,
                });
            }
        }
        let environment = self
            .source
            .environment(principal, session, offered_domains)?;
        if environment.domains() != offered_domains {
            return Err(ShellEnvironmentError::CapabilityMismatch);
        }
        let mut sessions = self.sessions.lock();
        if let Some(existing) = sessions.get(&session) {
            return if &existing.principal == principal
                && existing.environment.domains() == offered_domains
            {
                Ok(())
            } else {
                Err(ShellEnvironmentError::SessionIdentityMismatch)
            };
        }
        if sessions.len() >= self.limits.maximum_ready_sessions {
            return Err(ShellEnvironmentError::SessionCapacity {
                maximum: self.limits.maximum_ready_sessions,
            });
        }
        sessions.insert(
            session,
            PreparedSession {
                principal: principal.clone(),
                environment,
                ready: false,
            },
        );
        Ok(())
    }

    pub fn is_ready(&self, session: SessionId) -> bool {
        self.sessions
            .lock()
            .get(&session)
            .is_some_and(|session| session.ready)
    }

    pub fn close_session(&self, session: SessionId) -> bool {
        self.sessions.lock().remove(&session).is_some()
    }
}

impl Provider for ShellProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        if request.action.as_ref() != "ready" {
            return Err(invalid(&request, "unknown action"));
        }
        if request.correlation_id.is_some() {
            return Err(invalid(&request, "`shell.ready` must not carry an id"));
        }
        if request
            .payload
            .as_object()
            .is_none_or(|payload| !payload.is_empty())
        {
            return Err(invalid(
                &request,
                "`shell.ready` must carry no payload fields",
            ));
        }

        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.get_mut(&request.session) else {
            return Err(ProviderError::Denied {
                domain: Arc::from("shell"),
                action: Arc::clone(&request.action),
                reason: Arc::from("mapped session environment is not prepared"),
            });
        };
        if session.principal != request.principal {
            return Err(ProviderError::Denied {
                domain: Arc::from("shell"),
                action: Arc::clone(&request.action),
                reason: Arc::from("mapped session identity does not match"),
            });
        }
        if session.ready {
            return Ok(ProviderCall::completed(None));
        }
        session.ready = true;
        Ok(ProviderCall::completed(Some(session.environment.init())))
    }
}

fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from("shell"),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn validate_environment_limits(
    limits: ShellEnvironmentLimits,
) -> Result<(), ShellEnvironmentError> {
    if [
        limits.maximum_domains,
        limits.maximum_services,
        limits.maximum_service_name_bytes,
        limits.maximum_response_bytes,
    ]
    .contains(&0)
    {
        return Err(ShellEnvironmentError::InvalidLimits);
    }
    Ok(())
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ShellEnvironmentError {
    #[error("shell environment limits must be finite and non-zero")]
    InvalidLimits,
    #[error("shell environment has {actual} domains; the maximum is {maximum}")]
    DomainCapacity { actual: usize, maximum: usize },
    #[error("shell environment has {actual} services; the maximum is {maximum}")]
    ServiceCapacity { actual: usize, maximum: usize },
    #[error("shell service names must be non-empty, control-free, and bounded")]
    InvalidServiceName,
    #[error("shell.init exceeds the {maximum} byte response limit")]
    ResponseTooLarge { maximum: usize },
    #[error("shell session environment must include the foundational shell domain")]
    MissingFoundationalShell,
    #[error("shell environment does not equal the exact negotiated capability set")]
    CapabilityMismatch,
    #[error("shell session identity or capability plan changed after preparation")]
    SessionIdentityMismatch,
    #[error("shell session capacity is full at {maximum}")]
    SessionCapacity { maximum: usize },
}

#[cfg(test)]
mod tests {
    use nmp_native_runtime_core::{ResourceClass, ResourceLimits, ResourceTracker};
    use serde_json::{Value, json};

    use super::*;

    #[derive(Debug)]
    struct FixedEnvironment;

    impl ShellEnvironmentSource for FixedEnvironment {
        fn environment(
            &self,
            _principal: &Principal,
            _session: SessionId,
            offered_domains: &BTreeSet<Capability>,
        ) -> Result<ShellEnvironment, ShellEnvironmentError> {
            ShellEnvironment::new(
                offered_domains.iter().cloned(),
                [Arc::from("settings")],
                ShellEnvironmentLimits::default(),
            )
        }
    }

    fn principal() -> Principal {
        Principal::new("a".repeat(64), "napplet", "b".repeat(64)).unwrap()
    }

    fn call(
        provider: &ShellProvider,
        resources: &ResourceTracker,
        session: u64,
        id: Option<&str>,
        payload: Value,
    ) -> Result<ProviderCall, ProviderError> {
        provider.call(ProviderRequest {
            principal: principal(),
            session: SessionId(session),
            action: Arc::from("ready"),
            correlation_id: id.map(Arc::from),
            payload,
            work: resources
                .admit(
                    SessionId(session),
                    Some(Capability::new("shell").unwrap()),
                    ResourceClass::ProviderCall,
                )
                .unwrap(),
        })
    }

    fn offered() -> BTreeSet<Capability> {
        BTreeSet::from([
            Capability::new("shell").unwrap(),
            Capability::new("storage").unwrap(),
        ])
    }

    #[test]
    fn first_ready_emits_one_uncorrelated_init_and_duplicate_is_idempotent() {
        let provider =
            ShellProvider::new(Arc::new(FixedEnvironment), ShellProviderLimits::default()).unwrap();
        let resources = ResourceTracker::new(ResourceLimits::default()).unwrap();
        provider
            .prepare_session(&principal(), SessionId(1), &offered())
            .unwrap();
        let first = call(&provider, &resources, 1, None, json!({})).unwrap();
        assert_eq!(
            first.response.unwrap().decode().unwrap(),
            json!({
                "type":"shell.init",
                "capabilities":{"domains":["shell","storage"]},
                "services":["settings"]
            })
        );
        assert!(provider.is_ready(SessionId(1)));
        assert!(
            call(&provider, &resources, 1, None, json!({}))
                .unwrap()
                .response
                .is_none()
        );
        assert_eq!(resources.census().admitted, 0);
    }

    #[test]
    fn ready_payload_and_id_are_exact() {
        let provider =
            ShellProvider::new(Arc::new(FixedEnvironment), ShellProviderLimits::default()).unwrap();
        let resources = ResourceTracker::new(ResourceLimits::default()).unwrap();
        provider
            .prepare_session(&principal(), SessionId(1), &offered())
            .unwrap();
        assert!(matches!(
            call(&provider, &resources, 1, Some("id"), json!({})),
            Err(ProviderError::InvalidPayload { .. })
        ));
        assert!(matches!(
            call(&provider, &resources, 1, None, json!({"capabilities":[]})),
            Err(ProviderError::InvalidPayload { .. })
        ));
        assert!(!provider.is_ready(SessionId(1)));
    }

    #[test]
    fn readiness_capacity_is_bounded_and_close_releases_it() {
        let provider = ShellProvider::new(
            Arc::new(FixedEnvironment),
            ShellProviderLimits {
                maximum_ready_sessions: 1,
            },
        )
        .unwrap();
        let resources = ResourceTracker::new(ResourceLimits::default()).unwrap();
        provider
            .prepare_session(&principal(), SessionId(1), &offered())
            .unwrap();
        call(&provider, &resources, 1, None, json!({})).unwrap();
        assert!(matches!(
            provider.prepare_session(&principal(), SessionId(2), &offered()),
            Err(ShellEnvironmentError::SessionCapacity { .. })
        ));
        assert!(provider.close_session(SessionId(1)));
        provider
            .prepare_session(&principal(), SessionId(2), &offered())
            .unwrap();
        call(&provider, &resources, 2, None, json!({})).unwrap();
        assert!(provider.is_ready(SessionId(2)));
    }

    #[test]
    fn descriptor_matches_registry_only_handshake_inventory() {
        let provider =
            ShellProvider::new(Arc::new(FixedEnvironment), ShellProviderLimits::default()).unwrap();
        let inventory: Value = serde_json::from_str(include_str!(
            "../../../conformance/envelopes/inventory.json"
        ))
        .unwrap();
        let outbound = inventory["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| {
                entry["domain"] == "shell"
                    && entry["direction"] == "napplet-to-shell"
                    && entry["validator"] == "registry-only-handshake"
            })
            .map(|entry| {
                entry["type"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("shell.")
                    .unwrap()
            })
            .map(Arc::from)
            .collect::<BTreeSet<_>>();
        assert_eq!(provider.descriptor().actions, outbound);
        assert_eq!(
            provider.descriptor().protocol_versions,
            BTreeSet::from([Arc::from(PINNED_SHELL_PROTOCOL)])
        );
    }
}
