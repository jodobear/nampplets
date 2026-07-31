use std::{collections::BTreeMap, sync::Arc};

use nmp_native_nap_bridge::{ProviderError, ProviderRequest};
use nmp_native_runtime_core::BoundedJson;
use serde_json::Value;

use super::wire::{exact_object, invalid, optional_text, required_text, valid_slug, valid_text};
use super::*;

pub(super) fn validate_invocation(
    request: &ProviderRequest,
    limits: IntentProviderLimits,
) -> Result<ValidatedInvocation, ProviderError> {
    let outer = exact_object(request, &["request"])?;
    let object = outer
        .get("request")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(request, "`request` must be an object"))?;
    if object.keys().any(|field| {
        !matches!(
            field.as_str(),
            "archetype" | "action" | "convention" | "protocol" | "payload" | "handler" | "behavior"
        )
    }) {
        return Err(invalid(request, "unknown intent request field"));
    }
    let archetype: Arc<str> = Arc::from(required_text(
        request,
        object,
        "archetype",
        limits.maximum_text_bytes,
    )?);
    if !valid_slug(&archetype) {
        return Err(invalid(
            request,
            "`archetype` must be a lowercase role slug",
        ));
    }
    let action: Arc<str> = match object.get("action") {
        None => Arc::from("open"),
        Some(Value::String(action))
            if valid_text(action, limits.maximum_text_bytes) && valid_slug(action) =>
        {
            Arc::from(action.as_str())
        }
        _ => return Err(invalid(request, "`action` is invalid")),
    };
    // The vendored NAP-INTENT spec names this field `convention`, but the
    // `@napplet/nap` SDK actually deployed by published napplets (whose
    // `intent.open(archetype, payload, { protocol })` sugar spreads `opts`
    // directly onto the wire request) sends it as `protocol`. Accept either
    // spelling so real-world napplets built against that SDK aren't rejected.
    let named_convention = optional_text(request, object, "convention", limits.maximum_text_bytes)?;
    let protocol_alias = optional_text(request, object, "protocol", limits.maximum_text_bytes)?;
    if matches!(
        (&named_convention, &protocol_alias),
        (Some(convention), Some(protocol)) if convention != protocol
    ) {
        return Err(invalid(
            request,
            "`convention` and its `protocol` alias must agree",
        ));
    }
    let expected_convention = format!("napplet:{archetype}/{action}");
    let convention = named_convention
        .or(protocol_alias)
        .unwrap_or_else(|| Arc::from(expected_convention.as_str()));
    // In 0.28 an explicit convention is independent handler routing identity,
    // not a value derived from the request's archetype/action pair.
    if !valid_queryless_convention(&convention, limits.maximum_text_bytes) {
        return Err(invalid(
            request,
            "intent convention must be a stable queryless napplet convention",
        ));
    }
    let handler_request = match object.get("handler") {
        None => IntentHandlerRequest::Default,
        Some(Value::String(value)) if value == "default" => IntentHandlerRequest::Default,
        Some(Value::String(value)) if value == "choose" => IntentHandlerRequest::Choose,
        Some(Value::String(value)) if valid_text(value, limits.maximum_text_bytes) => {
            IntentHandlerRequest::Specific(Arc::from(value.as_str()))
        }
        _ => return Err(invalid(request, "`handler` is invalid")),
    };
    let behavior = object
        .get("behavior")
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|_| invalid(request, "`behavior` is invalid"))
        })
        .transpose()?
        .unwrap_or_default();
    let payload = BoundedJson::from_value(
        object.get("payload").unwrap_or(&Value::Null),
        limits.maximum_payload_bytes,
    )
    .map_err(|_| invalid(request, "`payload` exceeds its byte limit"))?;
    Ok(ValidatedInvocation {
        archetype,
        action,
        convention: Some(convention),
        payload,
        handler_request,
        behavior,
    })
}

pub(super) fn matching_handlers(
    state: &IntentState,
    invocation: &ValidatedInvocation,
) -> Vec<RegisteredHandler> {
    state
        .handlers
        .get(&invocation.archetype)
        .into_iter()
        .flat_map(BTreeMap::values)
        .filter(|handler| {
            handler.declaration.actions.contains(&invocation.action)
                && invocation
                    .convention
                    .as_ref()
                    .is_none_or(|convention| handler.declaration.conventions.contains(convention))
        })
        .cloned()
        .collect()
}

pub(super) fn availability_locked(state: &IntentState, archetype: &str) -> IntentAvailability {
    let default = state.defaults.get(archetype);
    let candidates = state
        .handlers
        .get(archetype)
        .into_iter()
        .flat_map(BTreeMap::values)
        .map(|handler| candidate_view(handler, default == Some(&handler.principal)))
        .collect::<Vec<_>>();
    IntentAvailability {
        archetype: Arc::from(archetype),
        available: !candidates.is_empty(),
        has_default: default.is_some(),
        candidates,
    }
}

pub(super) fn candidate_view(handler: &RegisteredHandler, is_default: bool) -> IntentCandidate {
    IntentCandidate {
        d_tag: Arc::from(handler.principal.d_tag()),
        title: handler.declaration.title.clone(),
        actions: handler.declaration.actions.iter().cloned().collect(),
        conventions: handler.declaration.conventions.iter().cloned().collect(),
        is_default,
    }
}

pub(super) fn valid_declaration(
    declaration: &IntentHandlerDeclaration,
    limits: IntentProviderLimits,
) -> bool {
    valid_slug(&declaration.archetype)
        && declaration
            .title
            .as_deref()
            .is_none_or(|title| valid_text(title, limits.maximum_text_bytes))
        && !declaration.actions.is_empty()
        && declaration.actions.len() <= limits.maximum_actions_per_handler
        && declaration.actions.iter().all(|action| valid_slug(action))
        && declaration.conventions.len() <= limits.maximum_conventions_per_handler
        && declaration
            .conventions
            .iter()
            .all(|convention| valid_queryless_convention(convention, limits.maximum_text_bytes))
}

fn valid_queryless_convention(value: &str, maximum_bytes: usize) -> bool {
    let Some(path) = value.strip_prefix("napplet:") else {
        return false;
    };
    let Some((subject, action)) = path.split_once('/') else {
        return false;
    };
    valid_text(value, maximum_bytes)
        && !subject.is_empty()
        && !action.is_empty()
        && !action.contains('/')
        && [subject, action].into_iter().all(|component| {
            !component
                .chars()
                .any(|character| character.is_whitespace() || matches!(character, '?' | '#'))
        })
}

pub(super) fn validate_limits(
    limits: IntentProviderLimits,
) -> Result<(), IntentProviderBuildError> {
    if [
        limits.maximum_sessions,
        limits.maximum_handlers,
        limits.maximum_archetypes,
        limits.maximum_candidates_per_archetype,
        limits.maximum_actions_per_handler,
        limits.maximum_conventions_per_handler,
        limits.maximum_pending_per_session,
        limits.maximum_pending_total,
        limits.maximum_payload_bytes,
        limits.maximum_response_bytes,
        limits.maximum_correlation_id_bytes,
        limits.maximum_text_bytes,
        limits.maximum_native_handle_bytes,
    ]
    .contains(&0)
        || limits.maximum_pending_total < limits.maximum_pending_per_session
    {
        return Err(IntentProviderBuildError::InvalidLimits);
    }
    Ok(())
}
