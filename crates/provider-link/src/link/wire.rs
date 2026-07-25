use std::sync::Arc;

use nmp_native_nap_bridge::{ProviderCall, ProviderError, ProviderRequest};
use nmp_native_runtime_core::BoundedJson;
use serde_json::Value;

use super::url::{parse_label, validate_external_url};
use super::*;

impl LinkProvider {
    pub(super) fn open(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id: Arc<str> = Arc::from(correlation_id(&request, self.limits)?);
        let request_action = Arc::clone(&request.action);
        let object = request
            .payload
            .as_object()
            .ok_or_else(|| invalid(&request, "payload must be an object"))?;
        if !object.contains_key("url")
            || object
                .keys()
                .any(|key| !matches!(key.as_str(), "url" | "options"))
        {
            return Err(invalid(
                &request,
                "only `url` and optional `options` are allowed",
            ));
        }
        let url = object
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(&request, "`url` must be a string"))?;
        let label = parse_label(object.get("options"), self.limits.maximum_label_bytes)
            .map_err(|reason| invalid(&request, reason))?;
        let normalized_url = match validate_external_url(url, self.limits.maximum_url_bytes) {
            Ok(url) => url,
            Err(refusal) => {
                self.activity.record(LinkActivity {
                    principal: request.principal.clone(),
                    session: request.session,
                    outcome: LinkActivityOutcome::Denied,
                });
                return terminal_response(
                    &id,
                    LinkTerminal::Denied {
                        error: refusal.code(),
                    },
                    self.limits.maximum_response_bytes,
                    &request_action,
                );
            }
        };
        let policy_request = LinkPolicyRequest {
            principal: request.principal.clone(),
            session: request.session,
            normalized_url: Arc::clone(&normalized_url),
        };
        if self.policy.evaluate(&policy_request) == LinkPolicyDecision::Deny {
            self.activity.record(LinkActivity {
                principal: request.principal.clone(),
                session: request.session,
                outcome: LinkActivityOutcome::Denied,
            });
            return terminal_response(
                &id,
                LinkTerminal::Denied {
                    error: "blocked-by-policy",
                },
                self.limits.maximum_response_bytes,
                &request_action,
            );
        }

        let cancellation = request.work.cancellation().clone();
        let token =
            {
                let mut state = self.state.lock();
                ensure_ready_link_session(&state, &request)?;
                if state.pending.len() >= self.limits.maximum_pending_total
                    || state
                        .pending
                        .values()
                        .filter(|pending| pending.session == request.session)
                        .count()
                        >= self.limits.maximum_pending_per_session
                {
                    self.activity.record(LinkActivity {
                        principal: request.principal.clone(),
                        session: request.session,
                        outcome: LinkActivityOutcome::Refused,
                    });
                    return terminal_response(
                        &id,
                        LinkTerminal::Rejected {
                            error: "link operation capacity is full",
                        },
                        self.limits.maximum_response_bytes,
                        &request_action,
                    );
                }
                if state.pending.values().any(|pending| {
                    pending.session == request.session && pending.correlation_id == id
                }) {
                    return terminal_response(
                        &id,
                        LinkTerminal::Rejected {
                            error: "duplicate outstanding correlation id",
                        },
                        self.limits.maximum_response_bytes,
                        &request_action,
                    );
                }
                let Some(next) = state.next_token.checked_add(1) else {
                    return terminal_response(
                        &id,
                        LinkTerminal::Rejected {
                            error: "link operation id space is exhausted",
                        },
                        self.limits.maximum_response_bytes,
                        &request_action,
                    );
                };
                state.next_token = next;
                let token = LinkOperationToken(next);
                state.pending.insert(
                    token,
                    PendingLink {
                        principal: request.principal.clone(),
                        session: request.session,
                        correlation_id: Arc::clone(&id),
                        native_handle: None,
                        work: request.work,
                    },
                );
                token
            };
        let native_request = NativeLinkOpenRequest {
            token,
            principal: request.principal.clone(),
            session: request.session,
            normalized_url,
            label,
            confirmation_required: true,
            cancellation,
        };
        match self.opener.try_open(native_request) {
            Ok(handle) => {
                if !valid_text(&handle, self.limits.maximum_native_handle_bytes) {
                    let pending = self.state.lock().pending.remove(&token);
                    if let Some(pending) = pending {
                        drop(pending.work);
                    }
                    self.opener.cancel(&handle);
                    self.activity.record(LinkActivity {
                        principal: request.principal,
                        session: request.session,
                        outcome: LinkActivityOutcome::Refused,
                    });
                    return terminal_response(
                        &id,
                        LinkTerminal::Rejected {
                            error: "native opener returned an invalid handle",
                        },
                        self.limits.maximum_response_bytes,
                        &request_action,
                    );
                }
                let retained = {
                    let mut state = self.state.lock();
                    state.pending.get_mut(&token).is_some_and(|pending| {
                        pending.native_handle = Some(Arc::clone(&handle));
                        true
                    })
                };
                if !retained {
                    self.opener.cancel(&handle);
                }
                self.activity.record(LinkActivity {
                    principal: request.principal,
                    session: request.session,
                    outcome: LinkActivityOutcome::Started,
                });
                Ok(ProviderCall::completed(None))
            }
            Err(error) => {
                let pending = self.state.lock().pending.remove(&token);
                if let Some(pending) = pending {
                    drop(pending.work);
                }
                self.activity.record(LinkActivity {
                    principal: request.principal.clone(),
                    session: request.session,
                    outcome: LinkActivityOutcome::Refused,
                });
                let error = error.to_string();
                terminal_response(
                    &id,
                    LinkTerminal::Rejected { error: &error },
                    self.limits.maximum_response_bytes,
                    &request_action,
                )
            }
        }
    }
}

fn ensure_ready_link_session(
    state: &LinkState,
    request: &ProviderRequest,
) -> Result<(), ProviderError> {
    match state.sessions.get(&request.session) {
        Some(session) if session.principal == request.principal && session.ready => Ok(()),
        Some(_) => Err(denied(request, "mapped link session is not ready")),
        None => Err(denied(request, "link session is not mapped")),
    }
}

fn correlation_id(
    request: &ProviderRequest,
    limits: LinkProviderLimits,
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

pub(super) fn valid_text(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && !value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinkTerminal<'a> {
    Opened,
    Denied {
        error: &'a str,
    },
    /// The pinned shim rejects this terminal because it deliberately omits a
    /// public status rather than manufacturing `failed` or `cancelled`.
    Rejected {
        error: &'a str,
    },
}

pub(super) fn terminal_fields(
    correlation_id: &str,
    terminal: LinkTerminal<'_>,
) -> serde_json::Map<String, Value> {
    let mut fields =
        serde_json::Map::from_iter([("id".to_owned(), Value::String(correlation_id.to_owned()))]);
    match terminal {
        LinkTerminal::Opened => {
            fields.insert("status".to_owned(), Value::String("opened".to_owned()));
        }
        LinkTerminal::Denied { error } => {
            fields.insert("status".to_owned(), Value::String("denied".to_owned()));
            fields.insert("error".to_owned(), Value::String(error.to_owned()));
        }
        LinkTerminal::Rejected { error } => {
            fields.insert("error".to_owned(), Value::String(error.to_owned()));
        }
    }
    fields
}

fn terminal_response(
    correlation_id: &str,
    terminal: LinkTerminal<'_>,
    maximum_bytes: usize,
    action: &Arc<str>,
) -> Result<ProviderCall, ProviderError> {
    let mut fields = terminal_fields(correlation_id, terminal);
    fields.insert(
        "type".to_owned(),
        Value::String("link.open.result".to_owned()),
    );
    let value = Value::Object(fields);
    BoundedJson::from_value(&value, maximum_bytes)
        .map(|value| ProviderCall::completed(Some(value)))
        .map_err(|_| ProviderError::Failed {
            domain: Arc::from(LINK_DOMAIN),
            action: Arc::clone(action),
            reason: Arc::from("response exceeds the configured byte limit"),
        })
}

pub(super) fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(LINK_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn denied(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Denied {
        domain: Arc::from(LINK_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

pub(super) fn lifecycle_error(reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(LINK_DOMAIN),
        action: Arc::from("lifecycle"),
        reason: reason.into(),
    }
}
