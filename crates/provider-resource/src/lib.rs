//! Hardened, bounded NAP-RESOURCE byte broker.
//!
//! The provider never gives a napplet a network primitive. Rust validates the
//! exact URL, DNS answers, redirect chain, response size, MIME signature,
//! Blossom digest, SVG raster result, rate/concurrency quota and lifecycle.
//! The injected [`ResourceNetwork`] and [`SvgRasterizer`] execute only the raw
//! bounded capabilities Rust requests.
//!
//! JSON has no byte-string type, so the native provider envelope represents
//! the NAP `bstr` `blob` field as standard padded base64. The trusted web
//! projection must turn that field into a `Blob` before posting the terminal
//! envelope into the sandbox. The untrusted napplet never observes the base64
//! transport representation.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushError, ProviderPushSender, ProviderPushTermination, ProviderRequest,
    ProviderSession, ProviderSessionContext, ProviderSessionEnd,
};
use nmp_native_runtime_core::{BoundedJson, Cancellation, Capability, Principal, SessionId};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use url::Url;

mod policy;
mod types;

use policy::{AcquisitionContext, validate_blossom_server};
pub use types::*;

pub const DOMAIN: &str = "resource";
pub const PINNED_NAP_PROTOCOL: &str = "napplet-web@0.28.0";
pub const NATIVE_BLOB_ENCODING: &str = "base64-standard-padded";

#[derive(Debug)]
pub struct ResourceProvider {
    descriptor: ProviderDescriptor,
    shared: Arc<ResourceShared>,
}

#[derive(Debug)]
struct ResourceShared {
    network: Arc<dyn ResourceNetwork>,
    rasterizer: Arc<dyn SvgRasterizer>,
    clock: Arc<dyn ResourceClock>,
    activity: Arc<dyn ResourceActivitySink>,
    limits: ResourceProviderLimits,
    blossom_servers: Arc<[Url]>,
    state: Mutex<ResourceState>,
}

#[derive(Debug, Default)]
struct ResourceState {
    sessions: BTreeMap<SessionId, ResourceSession>,
    principal_in_flight: BTreeMap<Principal, usize>,
    rate: BTreeMap<Principal, RateBucket>,
    total_in_flight: usize,
    next_token: u64,
    closed: bool,
}

#[derive(Debug)]
struct ResourceSession {
    principal: Principal,
    outbound: ProviderPushSender,
    ready: bool,
    active: BTreeMap<Arc<str>, ActiveRequest>,
}

#[derive(Clone, Debug)]
struct ActiveRequest {
    token: u64,
    cancellation: Cancellation,
    url_count: usize,
}

#[derive(Clone, Copy, Debug)]
struct RateBucket {
    tokens_milli: u64,
    updated_at_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestKind {
    Bytes,
    BytesMany,
}

impl RequestKind {
    fn action(self) -> ResourceActivityAction {
        match self {
            Self::Bytes => ResourceActivityAction::Bytes,
            Self::BytesMany => ResourceActivityAction::BytesMany,
        }
    }

    fn result_type(self) -> &'static str {
        match self {
            Self::Bytes => "resource.bytes.result",
            Self::BytesMany => "resource.bytesMany.result",
        }
    }

    fn error_type(self) -> &'static str {
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

impl ResourceShared {
    fn wire_limit(&self) -> usize {
        self.limits
            .maximum_bulk_response_bytes
            .saturating_mul(2)
            .saturating_add(64 * 1024)
    }

    fn open_session(&self, session: ProviderSession) -> Result<(), ProviderError> {
        if session.outbound.domain().as_str() != DOMAIN
            || session.outbound.session() != session.context.session
        {
            return Err(lifecycle_error(
                "outbound resource lane does not match mapped session",
            ));
        }
        let mut state = self.state.lock();
        if state.closed {
            return Err(lifecycle_error("resource provider is closed"));
        }
        if let Some(existing) = state.sessions.get(&session.context.session) {
            return if existing.principal == session.context.principal
                && existing.outbound.source_window() == session.context.source_window
            {
                Ok(())
            } else {
                Err(lifecycle_error("mapped resource session identity changed"))
            };
        }
        if state.sessions.len() >= self.limits.maximum_sessions {
            return Err(lifecycle_error("resource session capacity is full"));
        }
        state.sessions.insert(
            session.context.session,
            ResourceSession {
                principal: session.context.principal,
                outbound: session.outbound,
                ready: false,
                active: BTreeMap::new(),
            },
        );
        Ok(())
    }

    fn ready_session(&self, context: &ProviderSessionContext) -> Result<(), ProviderError> {
        let mut state = self.state.lock();
        let session = state
            .sessions
            .get_mut(&context.session)
            .ok_or_else(|| lifecycle_error("resource session was not opened"))?;
        if session.principal != context.principal
            || session.outbound.source_window() != context.source_window
        {
            return Err(lifecycle_error("mapped resource session identity changed"));
        }
        session.ready = true;
        Ok(())
    }

    fn validate_call_context(&self, request: &ProviderRequest) -> Result<(), ProviderError> {
        let state = self.state.lock();
        let session = state
            .sessions
            .get(&request.session)
            .ok_or_else(|| failed(request, "resource session is not open"))?;
        if session.principal != request.principal || !session.ready {
            return Err(failed(
                request,
                "resource session is not ready for this exact principal",
            ));
        }
        Ok(())
    }

    fn reserve(
        &self,
        principal: &Principal,
        session_id: SessionId,
        id: Arc<str>,
        cancellation: Cancellation,
        url_count: usize,
    ) -> Result<u64, ResourceFailure> {
        let now = self.clock.monotonic_millis();
        let mut state = self.state.lock();
        if state.closed {
            return Err(ResourceFailure::new(
                ResourceErrorCode::NetworkError,
                "resource provider is closed",
            ));
        }
        {
            let session = state.sessions.get(&session_id).ok_or_else(|| {
                ResourceFailure::new(
                    ResourceErrorCode::InvalidRequest,
                    "resource session is not open",
                )
            })?;
            if &session.principal != principal || !session.ready {
                return Err(ResourceFailure::new(
                    ResourceErrorCode::BlockedByPolicy,
                    "resource session identity is not ready",
                ));
            }
            if session.active.contains_key(&id) {
                return Err(ResourceFailure::new(
                    ResourceErrorCode::InvalidRequest,
                    "resource request id is already active",
                ));
            }
        }
        if state.total_in_flight.saturating_add(url_count)
            > self.limits.maximum_total_in_flight_urls
            || state
                .principal_in_flight
                .get(principal)
                .copied()
                .unwrap_or(0)
                .saturating_add(url_count)
                > self.limits.maximum_in_flight_urls_per_napplet
        {
            return Err(ResourceFailure::new(
                ResourceErrorCode::BlockedByPolicy,
                "resource in-flight capacity is full",
            ));
        }
        if !take_rate_tokens(
            &mut state.rate,
            principal,
            now,
            url_count,
            self.limits.maximum_requests_per_napplet_per_minute,
        ) {
            return Err(ResourceFailure::new(
                ResourceErrorCode::BlockedByPolicy,
                "resource per-napplet rate limit was exceeded",
            ));
        }
        state.next_token = state.next_token.wrapping_add(1).max(1);
        let token = state.next_token;
        state.total_in_flight += url_count;
        *state
            .principal_in_flight
            .entry(principal.clone())
            .or_default() += url_count;
        state
            .sessions
            .get_mut(&session_id)
            .expect("session validated while lock is held")
            .active
            .insert(
                id,
                ActiveRequest {
                    token,
                    cancellation,
                    url_count,
                },
            );
        Ok(token)
    }

    fn run_request(
        self: Arc<Self>,
        principal: Principal,
        session: SessionId,
        id: Arc<str>,
        token: u64,
        kind: RequestKind,
        urls: Vec<Arc<str>>,
        cancellation: Cancellation,
    ) {
        let mut delivered_bytes = 0_usize;
        let context = AcquisitionContext {
            network: self.network.as_ref(),
            rasterizer: self.rasterizer.as_ref(),
            clock: self.clock.as_ref(),
            limits: self.limits,
            blossom_servers: &self.blossom_servers,
            cancellation: &cancellation,
        };
        let response = match kind {
            RequestKind::Bytes => match context.acquire(&urls[0]) {
                Ok(resource)
                    if resource.bytes.len() <= self.limits.maximum_blob_bytes_per_request =>
                {
                    delivered_bytes = resource.bytes.len();
                    json!({
                        "type": kind.result_type(),
                        "id": id,
                        "blob": STANDARD.encode(resource.bytes),
                        "mime": resource.mime,
                    })
                }
                Ok(_) => error_envelope(
                    kind.error_type(),
                    &id,
                    &ResourceFailure::new(
                        ResourceErrorCode::QuotaExceeded,
                        "resource Blob quota was exceeded",
                    ),
                ),
                Err(failure) => error_envelope(kind.error_type(), &id, &failure),
            },
            RequestKind::BytesMany => {
                let mut items = Vec::with_capacity(urls.len());
                let maximum_bulk_bytes = self
                    .limits
                    .maximum_bulk_response_bytes
                    .min(self.limits.maximum_blob_bytes_per_request);
                for url in &urls {
                    if cancellation.is_cancelled() {
                        break;
                    }
                    match context.acquire(url) {
                        Ok(resource)
                            if delivered_bytes.saturating_add(resource.bytes.len())
                                <= maximum_bulk_bytes =>
                        {
                            delivered_bytes += resource.bytes.len();
                            items.push(json!({
                                "url": url,
                                "ok": true,
                                "blob": STANDARD.encode(resource.bytes),
                                "mime": resource.mime,
                            }));
                        }
                        Ok(_) => items.push(json!({
                            "url": url,
                            "ok": false,
                            "error": ResourceErrorCode::QuotaExceeded.as_str(),
                            "message": "resource Blob quota was exceeded",
                        })),
                        Err(failure) => items.push(json!({
                            "url": url,
                            "ok": false,
                            "error": failure.code.as_str(),
                            "message": failure.message,
                        })),
                    }
                }
                json!({
                    "type": kind.result_type(),
                    "id": id,
                    "items": items,
                })
            }
        };
        if cancellation.is_cancelled() {
            self.release(session, &id, token);
            self.activity.record(ResourceActivity {
                principal,
                session,
                action: kind.action(),
                outcome: ResourceActivityOutcome::Cancelled,
                url_count: urls.len(),
                delivered_bytes: 0,
            });
            return;
        }
        let outbound = self.outbound_for(session, &principal, &id, token);
        let push = outbound.and_then(|outbound| {
            let envelope = BoundedJson::from_value(&response, self.wire_limit())
                .map_err(|error| ProviderPushError::Malformed(Arc::from(error.to_string())))?;
            outbound.push_envelope(&envelope, None).map(|_| ())
        });
        self.release(session, &id, token);
        match push {
            Ok(()) => self.activity.record(ResourceActivity {
                principal,
                session,
                action: kind.action(),
                outcome: ResourceActivityOutcome::Completed,
                url_count: urls.len(),
                delivered_bytes,
            }),
            Err(_) => {
                self.activity.record(ResourceActivity {
                    principal: principal.clone(),
                    session,
                    action: kind.action(),
                    outcome: ResourceActivityOutcome::PushRefused,
                    url_count: urls.len(),
                    delivered_bytes: 0,
                });
                self.fail_session_delivery(session, &principal);
            }
        }
    }

    fn outbound_for(
        &self,
        session: SessionId,
        principal: &Principal,
        id: &str,
        token: u64,
    ) -> Result<ProviderPushSender, ProviderPushError> {
        let state = self.state.lock();
        let session = state
            .sessions
            .get(&session)
            .ok_or(ProviderPushError::Closed)?;
        let active = session.active.get(id).ok_or(ProviderPushError::Closed)?;
        if &session.principal != principal || active.token != token {
            return Err(ProviderPushError::Revoked);
        }
        Ok(session.outbound.clone())
    }

    fn release(&self, session_id: SessionId, id: &str, token: u64) {
        let mut state = self.state.lock();
        let removed = state.sessions.get_mut(&session_id).and_then(|session| {
            session
                .active
                .get(id)
                .is_some_and(|active| active.token == token)
                .then(|| session.active.remove(id))
                .flatten()
                .map(|active| (session.principal.clone(), active.url_count))
        });
        if let Some((principal, url_count)) = removed {
            state.total_in_flight = state.total_in_flight.saturating_sub(url_count);
            decrement_principal(&mut state.principal_in_flight, &principal, url_count);
        }
    }

    fn cancel_request(&self, principal: &Principal, session: SessionId, id: &str) -> bool {
        let state = self.state.lock();
        let Some(session) = state.sessions.get(&session) else {
            return false;
        };
        if &session.principal != principal {
            return false;
        }
        session
            .active
            .get(id)
            .is_some_and(|request| request.cancellation.cancel())
    }

    fn remove_session(&self, context: &ProviderSessionContext) {
        let removed = {
            let mut state = self.state.lock();
            let Some(session) = state.sessions.get(&context.session) else {
                return;
            };
            if session.principal != context.principal
                || session.outbound.source_window() != context.source_window
            {
                return;
            }
            let session = state
                .sessions
                .remove(&context.session)
                .expect("session was present while lock is held");
            for active in session.active.values() {
                active.cancellation.cancel();
                state.total_in_flight = state.total_in_flight.saturating_sub(active.url_count);
                decrement_principal(
                    &mut state.principal_in_flight,
                    &session.principal,
                    active.url_count,
                );
            }
            let url_count = session.active.values().map(|active| active.url_count).sum();
            if !state
                .sessions
                .values()
                .any(|remaining| remaining.principal == session.principal)
            {
                state.rate.remove(&session.principal);
            }
            (session.principal, url_count)
        };
        self.activity.record(ResourceActivity {
            principal: removed.0,
            session: context.session,
            action: ResourceActivityAction::LifecycleCleanup,
            outcome: ResourceActivityOutcome::Cancelled,
            url_count: removed.1,
            delivered_bytes: 0,
        });
    }

    fn fail_session_delivery(&self, session_id: SessionId, principal: &Principal) {
        let mut state = self.state.lock();
        let Some(session) = state.sessions.remove(&session_id) else {
            return;
        };
        if &session.principal != principal {
            state.sessions.insert(session_id, session);
            return;
        }
        session
            .outbound
            .terminate(ProviderPushTermination::Backpressure);
        for active in session.active.values() {
            active.cancellation.cancel();
            state.total_in_flight = state.total_in_flight.saturating_sub(active.url_count);
            decrement_principal(
                &mut state.principal_in_flight,
                &session.principal,
                active.url_count,
            );
        }
        if !state
            .sessions
            .values()
            .any(|remaining| remaining.principal == session.principal)
        {
            state.rate.remove(&session.principal);
        }
    }

    fn close(&self) {
        let sessions = {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            state.total_in_flight = 0;
            state.principal_in_flight.clear();
            state.rate.clear();
            std::mem::take(&mut state.sessions)
        };
        for session in sessions.into_values() {
            for active in session.active.into_values() {
                active.cancellation.cancel();
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceCensus {
    pub sessions: usize,
    pub active_requests: usize,
    pub in_flight_urls: usize,
    pub closed: bool,
}

fn take_rate_tokens(
    buckets: &mut BTreeMap<Principal, RateBucket>,
    principal: &Principal,
    now_millis: u64,
    count: usize,
    maximum_per_minute: u32,
) -> bool {
    let capacity = u64::from(maximum_per_minute).saturating_mul(1_000);
    let bucket = buckets.entry(principal.clone()).or_insert(RateBucket {
        tokens_milli: capacity,
        updated_at_millis: now_millis,
    });
    let elapsed = now_millis.saturating_sub(bucket.updated_at_millis);
    let refill = elapsed
        .saturating_mul(u64::from(maximum_per_minute))
        .saturating_div(60);
    bucket.tokens_milli = bucket.tokens_milli.saturating_add(refill).min(capacity);
    bucket.updated_at_millis = now_millis;
    let requested = u64::try_from(count)
        .unwrap_or(u64::MAX)
        .saturating_mul(1_000);
    if bucket.tokens_milli < requested {
        return false;
    }
    bucket.tokens_milli -= requested;
    true
}

fn decrement_principal(
    counts: &mut BTreeMap<Principal, usize>,
    principal: &Principal,
    amount: usize,
) {
    let Some(count) = counts.get_mut(principal) else {
        return;
    };
    *count = count.saturating_sub(amount);
    if *count == 0 {
        counts.remove(principal);
    }
}

fn exact_payload<'a>(
    request: &'a ProviderRequest,
    fields: &[&str],
) -> Result<&'a Map<String, Value>, ProviderError> {
    let payload = request
        .payload
        .as_object()
        .ok_or_else(|| invalid(request, "payload must be an object"))?;
    if payload.len() != fields.len() || fields.iter().any(|field| !payload.contains_key(*field)) {
        return Err(invalid(request, "payload fields do not match the action"));
    }
    Ok(payload)
}

fn required_string<'a>(
    payload: &'a Map<String, Value>,
    field: &str,
    request: &ProviderRequest,
) -> Result<&'a str, ProviderError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(request, format!("{field} must be a string")))
}

fn correlation_id(
    request: &ProviderRequest,
    limits: ResourceProviderLimits,
) -> Result<Arc<str>, ProviderError> {
    let id = request
        .correlation_id
        .as_ref()
        .ok_or_else(|| invalid(request, "id is required"))?;
    if id.is_empty() || id.len() > limits.maximum_correlation_id_bytes {
        return Err(invalid(
            request,
            format!(
                "id must be 1..={} bytes",
                limits.maximum_correlation_id_bytes
            ),
        ));
    }
    Ok(Arc::clone(id))
}

fn error_envelope(message_type: &str, id: &str, failure: &ResourceFailure) -> Value {
    json!({
        "type": message_type,
        "id": id,
        "error": failure.code.as_str(),
        "message": failure.message,
    })
}

fn bounded_response(
    response: &Value,
    maximum: usize,
    request: &ProviderRequest,
) -> Result<BoundedJson, ProviderError> {
    BoundedJson::from_value(response, maximum)
        .map_err(|_| failed(request, "resource response exceeded its native wire bound"))
}

fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn failed(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn lifecycle_error(reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(DOMAIN),
        action: Arc::from("session.lifecycle"),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests;
