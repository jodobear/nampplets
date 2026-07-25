use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushSender, ProviderRequest, ProviderSession, ProviderSessionContext,
    ProviderSessionEnd,
};
use nmp_native_runtime_core::{Cancellation, Capability, Principal, SessionId};
use parking_lot::Mutex;
use serde_json::{Value, json};
use url::Url;

use crate::{
    DOMAIN, PINNED_NAP_PROTOCOL, ResourceActivity, ResourceActivityAction, ResourceActivityOutcome,
    ResourceActivitySink, ResourceCensus, ResourceClock, ResourceErrorCode, ResourceFailure,
    ResourceNetwork, ResourceProviderBuildError, ResourceProviderLimits, SvgRasterizer,
    policy::validate_blossom_server,
    supported_schemes, validate_limits,
    wire::{
        bounded_response, correlation_id, error_envelope, exact_payload, failed, invalid,
        required_string,
    },
};

#[derive(Debug)]
pub struct ResourceProvider {
    descriptor: ProviderDescriptor,
    shared: Arc<ResourceShared>,
}

#[derive(Debug)]
pub(crate) struct ResourceShared {
    pub(crate) network: Arc<dyn ResourceNetwork>,
    pub(crate) rasterizer: Arc<dyn SvgRasterizer>,
    pub(crate) clock: Arc<dyn ResourceClock>,
    pub(crate) activity: Arc<dyn ResourceActivitySink>,
    pub(crate) limits: ResourceProviderLimits,
    pub(crate) blossom_servers: Arc<[Url]>,
    pub(crate) state: Mutex<ResourceState>,
}

#[derive(Debug, Default)]
pub(crate) struct ResourceState {
    pub(crate) sessions: BTreeMap<SessionId, ResourceSession>,
    pub(crate) principal_in_flight: BTreeMap<Principal, usize>,
    pub(crate) rate: BTreeMap<Principal, RateBucket>,
    pub(crate) total_in_flight: usize,
    pub(crate) next_token: u64,
    pub(crate) closed: bool,
}

#[derive(Debug)]
pub(crate) struct ResourceSession {
    pub(crate) principal: Principal,
    pub(crate) outbound: ProviderPushSender,
    pub(crate) ready: bool,
    pub(crate) active: BTreeMap<Arc<str>, ActiveRequest>,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveRequest {
    pub(crate) token: u64,
    pub(crate) cancellation: Cancellation,
    pub(crate) url_count: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RateBucket {
    pub(crate) tokens_milli: u64,
    pub(crate) updated_at_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestKind {
    Bytes,
    BytesMany,
}

impl RequestKind {
    pub(crate) fn action(self) -> ResourceActivityAction {
        match self {
            Self::Bytes => ResourceActivityAction::Bytes,
            Self::BytesMany => ResourceActivityAction::BytesMany,
        }
    }

    pub(crate) fn result_type(self) -> &'static str {
        match self {
            Self::Bytes => "resource.bytes.result",
            Self::BytesMany => "resource.bytesMany.result",
        }
    }

    pub(crate) fn error_type(self) -> &'static str {
        match self {
            Self::Bytes => "resource.bytes.error",
            Self::BytesMany => "resource.bytesMany.error",
        }
    }
}

impl ResourceProvider {
    pub fn new(
        network: Arc<dyn ResourceNetwork>,
        rasterizer: Arc<dyn SvgRasterizer>,
        clock: Arc<dyn ResourceClock>,
        activity: Arc<dyn ResourceActivitySink>,
        limits: ResourceProviderLimits,
        blossom_servers: impl IntoIterator<Item = impl Into<Arc<str>>>,
    ) -> Result<Self, ResourceProviderBuildError> {
        if !validate_limits(limits) {
            return Err(ResourceProviderBuildError::InvalidLimits);
        }
        let raw_servers = blossom_servers
            .into_iter()
            .map(Into::into)
            .collect::<Vec<Arc<str>>>();
        if raw_servers.is_empty() {
            return Err(ResourceProviderBuildError::MissingBlossomServer);
        }
        if raw_servers.len() > limits.maximum_blossom_servers {
            return Err(ResourceProviderBuildError::InvalidLimits);
        }
        let mut servers = Vec::with_capacity(raw_servers.len());
        for server in raw_servers {
            if server.len() > limits.maximum_url_bytes {
                return Err(ResourceProviderBuildError::InvalidBlossomServer {
                    server,
                    reason: Arc::from("server URL exceeds the resource URL byte limit"),
                });
            }
            let parsed = validate_blossom_server(&server).map_err(|failure| {
                ResourceProviderBuildError::InvalidBlossomServer {
                    server: Arc::clone(&server),
                    reason: failure.message,
                }
            })?;
            servers.push(parsed);
        }
        Ok(Self {
            descriptor: ProviderDescriptor {
                domain: Capability::new(DOMAIN).expect("static resource capability is valid"),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)]),
                actions: ["info", "bytes", "bytesMany", "cancel"]
                    .into_iter()
                    .map(Arc::from)
                    .collect(),
                sensitive: true,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            shared: Arc::new(ResourceShared {
                network,
                rasterizer,
                clock,
                activity,
                limits,
                blossom_servers: Arc::from(servers),
                state: Mutex::new(ResourceState::default()),
            }),
        })
    }

    pub fn close(&self) {
        self.shared.close();
    }

    pub fn census(&self) -> ResourceCensus {
        let state = self.shared.state.lock();
        ResourceCensus {
            sessions: state.sessions.len(),
            active_requests: state
                .sessions
                .values()
                .map(|session| session.active.len())
                .sum(),
            in_flight_urls: state.total_in_flight,
            closed: state.closed,
        }
    }

    fn info(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.shared.limits)?;
        if exact_payload(&request, &[]).is_err() {
            return self.completed_error(
                &request,
                "resource.info.error",
                ResourceActivityAction::Info,
                ResourceFailure::new(
                    ResourceErrorCode::InvalidRequest,
                    "resource.info accepts no payload fields",
                ),
            );
        }
        let schemes = supported_schemes()
            .into_iter()
            .map(|scheme| {
                json!({
                    "scheme": scheme.as_str(),
                    "enabled": true,
                })
            })
            .collect::<Vec<_>>();
        let response = bounded_response(
            &json!({
                "type": "resource.info.result",
                "id": id,
                "info": {
                    "schemes": schemes,
                    "maxBytes": self.shared.limits.maximum_response_bytes,
                    "maxUrls": self.shared.limits.maximum_urls_per_bulk,
                },
            }),
            self.shared.wire_limit(),
            &request,
        )?;
        self.shared.activity.record(ResourceActivity {
            principal: request.principal,
            session: request.session,
            action: ResourceActivityAction::Info,
            outcome: ResourceActivityOutcome::Completed,
            url_count: 0,
            delivered_bytes: 0,
        });
        Ok(ProviderCall::completed(Some(response)))
    }

    fn bytes(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        correlation_id(&request, self.shared.limits)?;
        let payload = match exact_payload(&request, &["url"]) {
            Ok(payload) => payload,
            Err(_) => {
                return self.top_level_error(
                    &request,
                    RequestKind::Bytes,
                    ResourceFailure::new(
                        ResourceErrorCode::InvalidRequest,
                        "resource.bytes requires exactly one url field",
                    ),
                );
            }
        };
        let url = match required_string(payload, "url", &request) {
            Ok(url) => url,
            Err(_) => {
                return self.top_level_error(
                    &request,
                    RequestKind::Bytes,
                    ResourceFailure::new(
                        ResourceErrorCode::InvalidRequest,
                        "resource.bytes url must be a string",
                    ),
                );
            }
        };
        let url: Arc<str> = Arc::from(url);
        self.start(request, RequestKind::Bytes, vec![url])
    }

    fn bytes_many(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        correlation_id(&request, self.shared.limits)?;
        let payload = match exact_payload(&request, &["urls"]) {
            Ok(payload) => payload,
            Err(_) => {
                return self.top_level_error(
                    &request,
                    RequestKind::BytesMany,
                    ResourceFailure::new(
                        ResourceErrorCode::InvalidRequest,
                        "resource.bytesMany requires exactly one urls field",
                    ),
                );
            }
        };
        let Some(values) = payload.get("urls").and_then(Value::as_array) else {
            return self.top_level_error(
                &request,
                RequestKind::BytesMany,
                ResourceFailure::new(
                    ResourceErrorCode::InvalidRequest,
                    "resource.bytesMany urls must be an array",
                ),
            );
        };
        if values.is_empty() {
            return self.top_level_error(
                &request,
                RequestKind::BytesMany,
                ResourceFailure::new(ResourceErrorCode::InvalidRequest, "urls must be non-empty"),
            );
        }
        if values.len() > self.shared.limits.maximum_urls_per_bulk {
            return self.top_level_error(
                &request,
                RequestKind::BytesMany,
                ResourceFailure::new(
                    ResourceErrorCode::TooLarge,
                    "bulk URL count exceeds its limit",
                ),
            );
        }
        let mut urls = Vec::with_capacity(values.len());
        for value in values {
            let Some(url) = value.as_str() else {
                return self.top_level_error(
                    &request,
                    RequestKind::BytesMany,
                    ResourceFailure::new(
                        ResourceErrorCode::InvalidRequest,
                        "every resource.bytesMany urls item must be a string",
                    ),
                );
            };
            urls.push(Arc::from(url));
        }
        self.start(request, RequestKind::BytesMany, urls)
    }

    fn start(
        &self,
        request: ProviderRequest,
        kind: RequestKind,
        urls: Vec<Arc<str>>,
    ) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.shared.limits)?;
        if urls
            .iter()
            .any(|url| url.is_empty() || url.len() > self.shared.limits.maximum_url_bytes)
        {
            return self.top_level_error(
                &request,
                kind,
                ResourceFailure::new(
                    ResourceErrorCode::InvalidRequest,
                    "resource URL is empty or exceeds its byte limit",
                ),
            );
        }
        let cancellation = request.work.cancellation().clone();
        let token = match self.shared.reserve(
            &request.principal,
            request.session,
            Arc::clone(&id),
            cancellation.clone(),
            urls.len(),
        ) {
            Ok(token) => token,
            Err(failure) => return self.top_level_error(&request, kind, failure),
        };
        self.shared.activity.record(ResourceActivity {
            principal: request.principal.clone(),
            session: request.session,
            action: kind.action(),
            outcome: ResourceActivityOutcome::Active,
            url_count: urls.len(),
            delivered_bytes: 0,
        });
        let shared = Arc::clone(&self.shared);
        let principal = request.principal.clone();
        let session = request.session;
        let id_for_worker = Arc::clone(&id);
        let worker = std::thread::Builder::new()
            .name(format!("nap-resource-{}-{token}", session.0))
            .spawn(move || {
                shared.run_request(
                    principal,
                    session,
                    id_for_worker,
                    token,
                    kind,
                    urls,
                    cancellation,
                );
            });
        if worker.is_err() {
            self.shared.release(request.session, &id, token);
            return self.top_level_error(
                &request,
                kind,
                ResourceFailure::new(
                    ResourceErrorCode::NetworkError,
                    "resource worker thread is unavailable",
                ),
            );
        }
        Ok(ProviderCall::streaming(None, request.work))
    }

    fn cancel(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.shared.limits)?;
        exact_payload(&request, &[])?;
        let cancelled = self
            .shared
            .cancel_request(&request.principal, request.session, &id);
        self.shared.activity.record(ResourceActivity {
            principal: request.principal,
            session: request.session,
            action: ResourceActivityAction::Cancel,
            outcome: if cancelled {
                ResourceActivityOutcome::Cancelled
            } else {
                ResourceActivityOutcome::Completed
            },
            url_count: 0,
            delivered_bytes: 0,
        });
        Ok(ProviderCall::completed(None))
    }

    fn top_level_error(
        &self,
        request: &ProviderRequest,
        kind: RequestKind,
        failure: ResourceFailure,
    ) -> Result<ProviderCall, ProviderError> {
        self.completed_error(request, kind.error_type(), kind.action(), failure)
    }

    fn completed_error(
        &self,
        request: &ProviderRequest,
        error_type: &str,
        action: ResourceActivityAction,
        failure: ResourceFailure,
    ) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(request, self.shared.limits)?;
        let response = error_envelope(error_type, &id, &failure);
        let response = bounded_response(&response, self.shared.wire_limit(), request)?;
        self.shared.activity.record(ResourceActivity {
            principal: request.principal.clone(),
            session: request.session,
            action,
            outcome: ResourceActivityOutcome::Refused(failure.code),
            url_count: 0,
            delivered_bytes: 0,
        });
        Ok(ProviderCall::completed(Some(response)))
    }
}

impl Provider for ResourceProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        if request.work.cancellation().is_cancelled() {
            return Err(failed(&request, "mapped session work was cancelled"));
        }
        self.shared.validate_call_context(&request)?;
        match request.action.as_ref() {
            "info" => self.info(request),
            "bytes" => self.bytes(request),
            "bytesMany" => self.bytes_many(request),
            "cancel" => self.cancel(request),
            _ => Err(invalid(&request, "unknown action")),
        }
    }

    fn session_opened(&self, session: ProviderSession) -> Result<(), ProviderError> {
        self.shared.open_session(session)
    }

    fn session_ready(&self, session: &ProviderSessionContext) -> Result<(), ProviderError> {
        self.shared.ready_session(session)
    }

    fn session_closed(&self, session: &ProviderSessionContext, _reason: ProviderSessionEnd) {
        self.shared.remove_session(session);
    }

    fn session_revoked(&self, session: &ProviderSessionContext) {
        self.shared.remove_session(session);
    }
}

impl Drop for ResourceProvider {
    fn drop(&mut self) {
        self.shared.close();
    }
}
