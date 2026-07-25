use std::sync::Arc;

use nmp_native_nap_bridge::{ProviderCall, ProviderError, ProviderRequest};
use nmp_native_runtime_core::{BoundedJson, Principal};
use serde_json::{Map, Value, json};

use super::validate::{
    availability_locked, candidate_view, matching_handlers, validate_invocation,
};
use super::*;

impl IntentProvider {
    pub(super) fn available(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderCall, ProviderError> {
        let id: Arc<str> = Arc::from(correlation_id(&request, self.limits)?);
        let fields = exact_object(&request, &["archetype"])?;
        let archetype = required_text(
            &request,
            fields,
            "archetype",
            self.limits.maximum_text_bytes,
        )?;
        let availability = self.availability_for(&request.principal, archetype);
        response(
            json!({
                "type":"intent.available.result",
                "id":id,
                "availability":availability
            }),
            self.limits.maximum_response_bytes,
            &request,
        )
    }

    pub(super) fn handlers(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        exact_object(&request, &[])?;
        let archetypes = self
            .state
            .lock()
            .handlers
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let handlers = archetypes
            .iter()
            .filter(|archetype| self.policy.allow_discovery(&request.principal, archetype))
            .map(|archetype| self.availability_for(&request.principal, archetype))
            .collect::<Vec<_>>();
        response(
            json!({"type":"intent.handlers.result","id":id,"handlers":handlers}),
            self.limits.maximum_response_bytes,
            &request,
        )
    }

    pub(super) fn invoke(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id: Arc<str> = Arc::from(correlation_id(&request, self.limits)?);
        let request_action = Arc::clone(&request.action);
        let caller = request.principal.clone();
        let session_id = request.session;
        let invocation = validate_invocation(&request, self.limits)?;
        let policy_request = IntentPolicyRequest {
            caller: request.principal.clone(),
            session: request.session,
            archetype: Arc::clone(&invocation.archetype),
            action: Arc::clone(&invocation.action),
            convention: invocation.convention.clone(),
            requested_handler: invocation.handler_request.clone(),
            behavior: invocation.behavior,
        };
        let decision = self.policy.evaluate(&policy_request);
        if !decision.allow
            || matches!(
                invocation.handler_request,
                IntentHandlerRequest::Specific(_)
            ) && !decision.allow_specific_handler
        {
            self.activity.record(IntentActivity {
                principal: request.principal.clone(),
                session: request.session,
                action: Arc::clone(&invocation.action),
                outcome: IntentActivityOutcome::Denied,
            });
            return intent_failure(&id, &invocation, "invoke denied", self.limits, &request);
        }
        let (handler, candidates) = {
            let state = self.state.lock();
            ensure_ready_intent_session(&state, &request)?;
            let candidates = matching_handlers(&state, &invocation);
            let selected = match &invocation.handler_request {
                IntentHandlerRequest::Default => state
                    .defaults
                    .get(&invocation.archetype)
                    .filter(|principal| {
                        candidates
                            .iter()
                            .any(|candidate| &candidate.principal == *principal)
                    })
                    .cloned()
                    .or_else(|| (candidates.len() == 1).then(|| candidates[0].principal.clone())),
                IntentHandlerRequest::Specific(d_tag) => candidates
                    .iter()
                    .find(|candidate| candidate.principal.d_tag() == d_tag.as_ref())
                    .map(|candidate| candidate.principal.clone()),
                IntentHandlerRequest::Choose => None,
            };
            (selected, candidates)
        };
        let handler = match invocation.handler_request {
            IntentHandlerRequest::Choose => {
                let public = candidates
                    .iter()
                    .map(|candidate| candidate_view(candidate, false))
                    .collect::<Vec<_>>();
                match self.chooser.try_choose(IntentChoiceRequest {
                    caller: request.principal.clone(),
                    session: request.session,
                    archetype: Arc::clone(&invocation.archetype),
                    action: Arc::clone(&invocation.action),
                    candidates: public,
                }) {
                    Ok(IntentChoice::Selected(d_tag)) => candidates
                        .iter()
                        .find(|candidate| candidate.principal.d_tag() == d_tag.as_ref())
                        .map(|candidate| candidate.principal.clone()),
                    Ok(IntentChoice::Cancelled) => {
                        return intent_failure(
                            &id,
                            &invocation,
                            "user cancelled",
                            self.limits,
                            &request,
                        );
                    }
                    Err(error) => {
                        return intent_failure(
                            &id,
                            &invocation,
                            &error.to_string(),
                            self.limits,
                            &request,
                        );
                    }
                }
            }
            _ => handler,
        };
        let Some(handler) = handler else {
            return intent_failure(&id, &invocation, "no handler", self.limits, &request);
        };

        let cancellation = request.work.cancellation().clone();
        let token =
            {
                let mut state = self.state.lock();
                ensure_ready_intent_session(&state, &request)?;
                if state.pending.len() >= self.limits.maximum_pending_total
                    || state
                        .pending
                        .values()
                        .filter(|pending| pending.session == request.session)
                        .count()
                        >= self.limits.maximum_pending_per_session
                {
                    return Err(denied(&request, "intent operation capacity is full"));
                }
                if state.pending.values().any(|pending| {
                    pending.session == request.session && pending.correlation_id == id
                }) {
                    return Err(invalid(&request, "duplicate outstanding correlation id"));
                }
                let next = state
                    .next_token
                    .checked_add(1)
                    .ok_or_else(|| denied(&request, "intent operation id space is exhausted"))?;
                state.next_token = next;
                let token = IntentOperationToken(next);
                state.pending.insert(
                    token,
                    PendingIntent {
                        caller: request.principal.clone(),
                        session: request.session,
                        correlation_id: Arc::clone(&id),
                        archetype: Arc::clone(&invocation.archetype),
                        action: Arc::clone(&invocation.action),
                        convention: invocation.convention.clone(),
                        handler: handler.clone(),
                        native_handle: None,
                        work: request.work,
                    },
                );
                token
            };
        let dispatch = NativeIntentDispatch {
            token,
            caller: caller.clone(),
            session: session_id,
            handler,
            archetype: invocation.archetype,
            action: invocation.action,
            convention: invocation.convention,
            payload: invocation.payload,
            behavior: invocation.behavior,
            confirmation_required: decision.confirmation_required,
            cancellation,
        };
        match self.dispatcher.try_dispatch(dispatch) {
            Ok(handle) => {
                if !valid_text(&handle, self.limits.maximum_native_handle_bytes) {
                    self.state.lock().pending.remove(&token);
                    self.dispatcher.cancel(&handle);
                    return Err(ProviderError::Failed {
                        domain: Arc::from(INTENT_DOMAIN),
                        action: request_action,
                        reason: Arc::from("native dispatcher returned an invalid handle"),
                    });
                }
                let retained = self
                    .state
                    .lock()
                    .pending
                    .get_mut(&token)
                    .is_some_and(|pending| {
                        pending.native_handle = Some(Arc::clone(&handle));
                        true
                    });
                if !retained {
                    self.dispatcher.cancel(&handle);
                }
                self.activity.record(IntentActivity {
                    principal: caller,
                    session: session_id,
                    action: policy_request.action,
                    outcome: IntentActivityOutcome::Started,
                });
                Ok(ProviderCall::completed(None))
            }
            Err(error) => {
                self.state.lock().pending.remove(&token);
                BoundedJson::from_value(
                    &json!({
                        "type":"intent.invoke.result",
                        "id":id,
                        "result":{
                            "ok":false,
                            "archetype":policy_request.archetype,
                            "action":policy_request.action,
                            "handled":false,
                            "error":error.to_string()
                        }
                    }),
                    self.limits.maximum_response_bytes,
                )
                .map(|response| ProviderCall::completed(Some(response)))
                .map_err(|_| ProviderError::Failed {
                    domain: Arc::from(INTENT_DOMAIN),
                    action: request_action,
                    reason: Arc::from("response exceeds the configured byte limit"),
                })
            }
        }
    }

    fn availability_for(&self, caller: &Principal, archetype: &str) -> IntentAvailability {
        if !self.policy.allow_discovery(caller, archetype) {
            return IntentAvailability {
                archetype: Arc::from(archetype),
                available: false,
                candidates: Vec::new(),
                has_default: false,
            };
        }
        let state = self.state.lock();
        availability_locked(&state, archetype)
    }

    pub(super) fn publish_changed(&self, archetype: &str) {
        let (availability, targets) = {
            let state = self.state.lock();
            let availability = availability_locked(&state, archetype);
            let targets = state
                .sessions
                .iter()
                .filter(|(_, session)| {
                    session.ready && self.policy.allow_discovery(&session.principal, archetype)
                })
                .map(|(id, session)| (*id, session.clone()))
                .collect::<Vec<_>>();
            (availability, targets)
        };
        for (session_id, session) in targets {
            let result = session.outbound.push(
                "intent.changed",
                Map::from_iter([(
                    "availability".to_owned(),
                    serde_json::to_value(&availability).expect("availability is serializable"),
                )]),
                Some(&format!("intent.changed:{archetype}")),
            );
            self.activity.record(IntentActivity {
                principal: session.principal,
                session: session_id,
                action: Arc::from("changed"),
                outcome: if result.is_ok() {
                    IntentActivityOutcome::CatalogChanged
                } else {
                    IntentActivityOutcome::PushRefused
                },
            });
        }
    }
}

fn intent_failure(
    id: &str,
    invocation: &ValidatedInvocation,
    error: &str,
    limits: IntentProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    response(
        json!({
            "type":"intent.invoke.result",
            "id":id,
            "result":{
                "ok":false,
                "archetype":invocation.archetype,
                "action":invocation.action,
                "handled":false,
                "error":error
            }
        }),
        limits.maximum_response_bytes,
        request,
    )
}

fn ensure_ready_intent_session(
    state: &IntentState,
    request: &ProviderRequest,
) -> Result<(), ProviderError> {
    match state.sessions.get(&request.session) {
        Some(session) if session.principal == request.principal && session.ready => Ok(()),
        Some(_) => Err(denied(request, "mapped intent session is not ready")),
        None => Err(denied(request, "intent session is not mapped")),
    }
}

pub(super) fn exact_object<'a>(
    request: &'a ProviderRequest,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, ProviderError> {
    let object = request
        .payload
        .as_object()
        .ok_or_else(|| invalid(request, "payload must be an object"))?;
    if object
        .keys()
        .any(|field| !allowed.contains(&field.as_str()))
    {
        return Err(invalid(request, "unknown payload field"));
    }
    Ok(object)
}

fn correlation_id(
    request: &ProviderRequest,
    limits: IntentProviderLimits,
) -> Result<&str, ProviderError> {
    let id = request
        .correlation_id
        .as_deref()
        .ok_or_else(|| invalid(request, "`id` is required"))?;
    if !valid_text(id, limits.maximum_correlation_id_bytes) {
        return Err(invalid(request, "`id` is invalid or too large"));
    }
    Ok(id)
}

pub(super) fn required_text<'a>(
    request: &ProviderRequest,
    object: &'a Map<String, Value>,
    field: &str,
    maximum_bytes: usize,
) -> Result<&'a str, ProviderError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| valid_text(value, maximum_bytes))
        .ok_or_else(|| {
            invalid(
                request,
                format!("`{field}` is required and must be bounded text"),
            )
        })
}

pub(super) fn optional_text(
    request: &ProviderRequest,
    object: &Map<String, Value>,
    field: &str,
    maximum_bytes: usize,
) -> Result<Option<Arc<str>>, ProviderError> {
    match object.get(field) {
        None => Ok(None),
        Some(Value::String(value)) if valid_text(value, maximum_bytes) => {
            Ok(Some(Arc::from(value.as_str())))
        }
        Some(_) => Err(invalid(request, format!("`{field}` must be bounded text"))),
    }
}

pub(super) fn valid_text(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && !value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
}

pub(super) fn valid_slug(value: &str) -> bool {
    valid_text(value, 256)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn response(
    value: Value,
    maximum_bytes: usize,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    BoundedJson::from_value(&value, maximum_bytes)
        .map(|value| ProviderCall::completed(Some(value)))
        .map_err(|_| failed(request, "response exceeds the configured byte limit"))
}

pub(super) fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(INTENT_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn denied(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Denied {
        domain: Arc::from(INTENT_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn failed(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(INTENT_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

pub(super) fn lifecycle_error(reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(INTENT_DOMAIN),
        action: Arc::from("lifecycle"),
        reason: reason.into(),
    }
}
