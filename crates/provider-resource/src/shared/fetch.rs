use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use nmp_native_nap_bridge::{ProviderPushError, ProviderPushSender, ProviderPushTermination};
use nmp_native_runtime_core::{BoundedJson, Cancellation, Principal, SessionId};
use serde_json::json;

use crate::{
    ResourceActivity, ResourceActivityOutcome, ResourceErrorCode, ResourceFailure,
    policy::AcquisitionContext,
    provider::{RequestKind, ResourceShared},
    wire::{decrement_principal, error_envelope},
};

impl ResourceShared {
    pub(crate) fn run_request(
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
}
