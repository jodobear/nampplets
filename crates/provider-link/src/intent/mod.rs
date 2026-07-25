use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushError, ProviderPushSender, ProviderRequest, ProviderSession,
    ProviderSessionContext, ProviderSessionEnd,
};
use nmp_native_runtime_core::{
    BoundedJson, Cancellation, Capability, Principal, SessionId, WorkLease,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::PINNED_NAP_PROTOCOL;

pub const INTENT_DOMAIN: &str = "intent";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntentProviderLimits {
    pub maximum_sessions: usize,
    pub maximum_handlers: usize,
    pub maximum_archetypes: usize,
    pub maximum_candidates_per_archetype: usize,
    pub maximum_actions_per_handler: usize,
    pub maximum_conventions_per_handler: usize,
    pub maximum_pending_per_session: usize,
    pub maximum_pending_total: usize,
    pub maximum_payload_bytes: usize,
    pub maximum_response_bytes: usize,
    pub maximum_correlation_id_bytes: usize,
    pub maximum_text_bytes: usize,
    pub maximum_native_handle_bytes: usize,
}

impl Default for IntentProviderLimits {
    fn default() -> Self {
        Self {
            maximum_sessions: 64,
            maximum_handlers: 256,
            maximum_archetypes: 128,
            maximum_candidates_per_archetype: 32,
            maximum_actions_per_handler: 32,
            maximum_conventions_per_handler: 32,
            maximum_pending_per_session: 8,
            maximum_pending_total: 128,
            maximum_payload_bytes: 128 * 1024,
            maximum_response_bytes: 256 * 1024,
            maximum_correlation_id_bytes: 1_024,
            maximum_text_bytes: 1_024,
            maximum_native_handle_bytes: 256,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentHandlerDeclaration {
    pub archetype: Arc<str>,
    pub title: Option<Arc<str>>,
    pub actions: BTreeSet<Arc<str>>,
    pub conventions: BTreeSet<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RegisteredHandler {
    principal: Principal,
    declaration: IntentHandlerDeclaration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentCandidate {
    pub d_tag: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<Arc<str>>,
    pub actions: Vec<Arc<str>>,
    pub conventions: Vec<Arc<str>>,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentAvailability {
    pub archetype: Arc<str>,
    pub available: bool,
    pub candidates: Vec<IntentCandidate>,
    pub has_default: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentBehavior {
    #[serde(default)]
    pub focus: bool,
    #[serde(default)]
    pub new_window: bool,
    #[serde(default)]
    pub reuse: bool,
}

#[derive(Clone, Debug)]
pub struct IntentPolicyRequest {
    pub caller: Principal,
    pub session: SessionId,
    pub archetype: Arc<str>,
    pub action: Arc<str>,
    pub convention: Option<Arc<str>>,
    pub requested_handler: IntentHandlerRequest,
    pub behavior: IntentBehavior,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntentHandlerRequest {
    Default,
    Choose,
    Specific(Arc<str>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentPolicyDecision {
    pub allow: bool,
    pub allow_specific_handler: bool,
    pub confirmation_required: bool,
    pub reveal_candidates: bool,
}

pub trait IntentPolicy: Send + Sync + fmt::Debug {
    fn evaluate(&self, request: &IntentPolicyRequest) -> IntentPolicyDecision;
    fn allow_discovery(&self, caller: &Principal, archetype: &str) -> bool;
}

/// Conservative policy: dispatch requires native confirmation, and callers
/// cannot target a concrete dTag without a product-specific policy.
#[derive(Debug, Default)]
pub struct ConfirmEveryIntent;

impl IntentPolicy for ConfirmEveryIntent {
    fn evaluate(&self, _request: &IntentPolicyRequest) -> IntentPolicyDecision {
        IntentPolicyDecision {
            allow: true,
            allow_specific_handler: false,
            confirmation_required: true,
            reveal_candidates: true,
        }
    }

    fn allow_discovery(&self, _caller: &Principal, _archetype: &str) -> bool {
        true
    }
}

#[derive(Clone, Debug)]
pub struct IntentChoiceRequest {
    pub caller: Principal,
    pub session: SessionId,
    pub archetype: Arc<str>,
    pub action: Arc<str>,
    pub candidates: Vec<IntentCandidate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntentChoice {
    Selected(Arc<str>),
    Cancelled,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum IntentChoiceError {
    #[error("native intent chooser is saturated")]
    Saturated,
    #[error("native intent chooser is unavailable")]
    Unavailable,
}

/// A nonblocking choice seam. It returns a raw dTag; Rust validates the choice
/// against the exact candidate set before dispatch.
pub trait IntentChooser: Send + Sync + fmt::Debug {
    fn try_choose(&self, request: IntentChoiceRequest) -> Result<IntentChoice, IntentChoiceError>;
}

#[derive(Debug, Default)]
pub struct CancelIntentChoice;

impl IntentChooser for CancelIntentChoice {
    fn try_choose(&self, _request: IntentChoiceRequest) -> Result<IntentChoice, IntentChoiceError> {
        Ok(IntentChoice::Cancelled)
    }
}

#[derive(Clone, Debug)]
pub struct NativeIntentDispatch {
    pub token: IntentOperationToken,
    pub caller: Principal,
    pub session: SessionId,
    pub handler: Principal,
    pub archetype: Arc<str>,
    pub action: Arc<str>,
    pub convention: Option<Arc<str>>,
    pub payload: BoundedJson,
    pub behavior: IntentBehavior,
    pub confirmation_required: bool,
    pub cancellation: Cancellation,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum NativeIntentStartError {
    #[error("native intent dispatcher is saturated")]
    Saturated,
    #[error("native intent dispatcher is unavailable")]
    Unavailable,
    #[error("native intent session is closed")]
    Closed,
}

/// Native executes the selected target and reports raw completion. It never
/// chooses or rewrites a handler, action, convention, or payload.
pub trait NativeIntentDispatcher: Send + Sync + fmt::Debug {
    fn try_dispatch(
        &self,
        request: NativeIntentDispatch,
    ) -> Result<Arc<str>, NativeIntentStartError>;
    fn cancel(&self, native_handle: &str);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntentOperationToken(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeIntentOutcome {
    Handled { window_id: Option<Arc<str>> },
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentActivityOutcome {
    Started,
    Handled,
    Cancelled,
    Denied,
    Refused,
    PushRefused,
    LifecycleCancelled,
    CatalogChanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentActivity {
    pub principal: Principal,
    pub session: SessionId,
    pub action: Arc<str>,
    pub outcome: IntentActivityOutcome,
}

pub trait IntentActivitySink: Send + Sync + fmt::Debug {
    fn record(&self, fact: IntentActivity);
}

#[derive(Debug, Default)]
pub struct NoopIntentActivity;

impl IntentActivitySink for NoopIntentActivity {
    fn record(&self, _fact: IntentActivity) {}
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum IntentProviderBuildError {
    #[error("intent provider limits must be finite, non-zero, and internally consistent")]
    InvalidLimits,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum IntentCatalogError {
    #[error("handler declaration is invalid")]
    InvalidDeclaration,
    #[error("intent handler or archetype capacity is full")]
    Capacity,
    #[error("a different exact-build principal already owns this dTag")]
    DTagCollision,
    #[error("default handler is not registered for that archetype")]
    UnknownDefault,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum IntentCompletionError {
    #[error("unknown or already-completed intent operation")]
    UnknownOperation,
    #[error("intent result delivery was refused: {0}")]
    Push(ProviderPushError),
}

#[derive(Debug)]
pub struct IntentProvider {
    policy: Arc<dyn IntentPolicy>,
    chooser: Arc<dyn IntentChooser>,
    dispatcher: Arc<dyn NativeIntentDispatcher>,
    activity: Arc<dyn IntentActivitySink>,
    limits: IntentProviderLimits,
    descriptor: ProviderDescriptor,
    state: Mutex<IntentState>,
}

#[derive(Debug, Default)]
struct IntentState {
    sessions: BTreeMap<SessionId, IntentSession>,
    handlers: BTreeMap<Arc<str>, BTreeMap<Principal, RegisteredHandler>>,
    defaults: BTreeMap<Arc<str>, Principal>,
    pending: BTreeMap<IntentOperationToken, PendingIntent>,
    next_token: u64,
}

#[derive(Clone, Debug)]
struct IntentSession {
    principal: Principal,
    outbound: ProviderPushSender,
    ready: bool,
}

#[derive(Debug)]
struct PendingIntent {
    caller: Principal,
    session: SessionId,
    correlation_id: Arc<str>,
    archetype: Arc<str>,
    action: Arc<str>,
    convention: Option<Arc<str>>,
    handler: Principal,
    native_handle: Option<Arc<str>>,
    work: WorkLease,
}

struct ValidatedInvocation {
    archetype: Arc<str>,
    action: Arc<str>,
    convention: Option<Arc<str>>,
    payload: BoundedJson,
    handler_request: IntentHandlerRequest,
    behavior: IntentBehavior,
}

impl IntentProvider {
    pub fn new(
        policy: Arc<dyn IntentPolicy>,
        chooser: Arc<dyn IntentChooser>,
        dispatcher: Arc<dyn NativeIntentDispatcher>,
        activity: Arc<dyn IntentActivitySink>,
        limits: IntentProviderLimits,
    ) -> Result<Self, IntentProviderBuildError> {
        validate_limits(limits)?;
        Ok(Self {
            policy,
            chooser,
            dispatcher,
            activity,
            limits,
            descriptor: ProviderDescriptor {
                domain: Capability::new(INTENT_DOMAIN).expect("static intent capability is valid"),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)]),
                actions: ["invoke", "available", "handlers"]
                    .into_iter()
                    .map(Arc::from)
                    .collect(),
                sensitive: true,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            state: Mutex::new(IntentState::default()),
        })
    }

    /// Trusted catalog mutation from a verified installed manifest.
    pub fn register_handler(
        &self,
        principal: Principal,
        declarations: Vec<IntentHandlerDeclaration>,
    ) -> Result<(), IntentCatalogError> {
        if declarations.is_empty()
            || declarations
                .iter()
                .any(|declaration| !valid_declaration(declaration, self.limits))
        {
            return Err(IntentCatalogError::InvalidDeclaration);
        }
        let mut affected = declarations
            .iter()
            .map(|declaration| Arc::clone(&declaration.archetype))
            .collect::<BTreeSet<_>>();
        {
            let mut state = self.state.lock();
            let owners = state
                .handlers
                .values()
                .flat_map(BTreeMap::values)
                .filter(|handler| handler.principal.d_tag() == principal.d_tag())
                .map(|handler| &handler.principal)
                .collect::<BTreeSet<_>>();
            if owners.iter().any(|owner| *owner != &principal) {
                return Err(IntentCatalogError::DTagCollision);
            }
            let mut prospective = state.handlers.clone();
            for (archetype, handlers) in &mut prospective {
                if handlers.remove(&principal).is_some() {
                    affected.insert(Arc::clone(archetype));
                }
            }
            prospective.retain(|_, handlers| !handlers.is_empty());
            let distinct_handlers = prospective
                .values()
                .flat_map(BTreeMap::keys)
                .collect::<BTreeSet<_>>();
            if distinct_handlers.len() >= self.limits.maximum_handlers {
                return Err(IntentCatalogError::Capacity);
            }
            for declaration in declarations {
                if !prospective.contains_key(&declaration.archetype)
                    && prospective.len() >= self.limits.maximum_archetypes
                {
                    return Err(IntentCatalogError::Capacity);
                }
                let handlers = prospective
                    .entry(Arc::clone(&declaration.archetype))
                    .or_default();
                if !handlers.contains_key(&principal)
                    && handlers.len() >= self.limits.maximum_candidates_per_archetype
                {
                    return Err(IntentCatalogError::Capacity);
                }
                handlers.insert(
                    principal.clone(),
                    RegisteredHandler {
                        principal: principal.clone(),
                        declaration,
                    },
                );
            }
            state.handlers = prospective;
            let defaults = state
                .defaults
                .iter()
                .filter(|(archetype, default)| {
                    state
                        .handlers
                        .get(*archetype)
                        .is_some_and(|candidates| candidates.contains_key(*default))
                })
                .map(|(archetype, default)| (Arc::clone(archetype), default.clone()))
                .collect();
            state.defaults = defaults;
        }
        for archetype in affected {
            self.publish_changed(&archetype);
        }
        Ok(())
    }

    pub fn unregister_handler(&self, principal: &Principal) {
        let affected = {
            let mut state = self.state.lock();
            let affected = state
                .handlers
                .iter_mut()
                .filter_map(|(archetype, handlers)| {
                    handlers.remove(principal).map(|_| Arc::clone(archetype))
                })
                .collect::<Vec<_>>();
            state.handlers.retain(|_, handlers| !handlers.is_empty());
            state.defaults.retain(|_, default| default != principal);
            affected
        };
        for archetype in affected {
            self.publish_changed(&archetype);
        }
    }

    /// Trusted, user-driven preference mutation. No NAP wire action reaches it.
    pub fn set_default(
        &self,
        archetype: &str,
        principal: Option<Principal>,
    ) -> Result<(), IntentCatalogError> {
        let archetype: Arc<str> = Arc::from(archetype);
        {
            let mut state = self.state.lock();
            match principal {
                Some(principal)
                    if state
                        .handlers
                        .get(&archetype)
                        .is_some_and(|handlers| handlers.contains_key(&principal)) =>
                {
                    state.defaults.insert(Arc::clone(&archetype), principal);
                }
                Some(_) => return Err(IntentCatalogError::UnknownDefault),
                None => {
                    state.defaults.remove(&archetype);
                }
            }
        }
        self.publish_changed(&archetype);
        Ok(())
    }

    pub fn pending_count(&self) -> usize {
        self.state.lock().pending.len()
    }

    pub fn complete(
        &self,
        token: IntentOperationToken,
        outcome: NativeIntentOutcome,
    ) -> Result<(), IntentCompletionError> {
        let (pending, outbound) = {
            let mut state = self.state.lock();
            let pending = state
                .pending
                .remove(&token)
                .ok_or(IntentCompletionError::UnknownOperation)?;
            let outbound = state
                .sessions
                .get(&pending.session)
                .filter(|session| session.ready && session.principal == pending.caller)
                .map(|session| session.outbound.clone());
            (pending, outbound)
        };
        drop(pending.work);
        let (ok, handled, error, window_id, activity_outcome) = match outcome {
            NativeIntentOutcome::Handled { window_id } => {
                (true, true, None, window_id, IntentActivityOutcome::Handled)
            }
            NativeIntentOutcome::Cancelled => (
                false,
                false,
                Some("user cancelled"),
                None,
                IntentActivityOutcome::Cancelled,
            ),
            NativeIntentOutcome::Failed => (
                false,
                false,
                Some("invoke failed"),
                None,
                IntentActivityOutcome::Refused,
            ),
        };
        self.activity.record(IntentActivity {
            principal: pending.caller.clone(),
            session: pending.session,
            action: Arc::clone(&pending.action),
            outcome: activity_outcome,
        });
        let Some(outbound) = outbound else {
            return Ok(());
        };
        let mut result = Map::from_iter([
            ("ok".to_owned(), Value::Bool(ok)),
            (
                "archetype".to_owned(),
                Value::String(pending.archetype.to_string()),
            ),
            (
                "action".to_owned(),
                Value::String(pending.action.to_string()),
            ),
            ("handled".to_owned(), Value::Bool(handled)),
            (
                "handler".to_owned(),
                Value::String(pending.handler.d_tag().to_owned()),
            ),
        ]);
        if let Some(convention) = &pending.convention {
            result.insert(
                "convention".to_owned(),
                Value::String(convention.to_string()),
            );
        }
        if let Some(window_id) = window_id {
            result.insert("windowId".to_owned(), Value::String(window_id.to_string()));
        }
        if let Some(error) = error {
            result.insert("error".to_owned(), Value::String(error.to_owned()));
        }
        outbound
            .push(
                "intent.invoke.result",
                Map::from_iter([
                    (
                        "id".to_owned(),
                        Value::String(pending.correlation_id.to_string()),
                    ),
                    ("result".to_owned(), Value::Object(result)),
                ]),
                None,
            )
            .map(|_| ())
            .map_err(|error| {
                self.activity.record(IntentActivity {
                    principal: pending.caller,
                    session: pending.session,
                    action: pending.action,
                    outcome: IntentActivityOutcome::PushRefused,
                });
                IntentCompletionError::Push(error)
            })
    }

    fn available(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
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

    fn handlers(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
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

    fn invoke(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
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

    fn publish_changed(&self, archetype: &str) {
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

    fn remove_session(&self, context: &ProviderSessionContext) {
        let cancelled = {
            let mut state = self.state.lock();
            if state
                .sessions
                .get(&context.session)
                .is_none_or(|session| session.principal != context.principal)
            {
                return;
            }
            state.sessions.remove(&context.session);
            let tokens = state
                .pending
                .iter()
                .filter_map(|(token, pending)| {
                    (pending.session == context.session).then_some(*token)
                })
                .collect::<Vec<_>>();
            tokens
                .into_iter()
                .filter_map(|token| state.pending.remove(&token))
                .collect::<Vec<_>>()
        };
        for pending in cancelled {
            pending.work.cancellation().cancel();
            if let Some(handle) = pending.native_handle {
                self.dispatcher.cancel(&handle);
            }
            self.activity.record(IntentActivity {
                principal: pending.caller,
                session: pending.session,
                action: pending.action,
                outcome: IntentActivityOutcome::LifecycleCancelled,
            });
        }
    }
}

impl Provider for IntentProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        match request.action.as_ref() {
            "invoke" => self.invoke(request),
            "available" => self.available(request),
            "handlers" => self.handlers(request),
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
                Err(lifecycle_error("mapped intent session identity changed"))
            };
        }
        if state.sessions.len() >= self.limits.maximum_sessions {
            return Err(lifecycle_error("intent session capacity is full"));
        }
        state.sessions.insert(
            session.context.session,
            IntentSession {
                principal: session.context.principal,
                outbound: session.outbound,
                ready: false,
            },
        );
        Ok(())
    }

    fn session_ready(&self, context: &ProviderSessionContext) -> Result<(), ProviderError> {
        let mut state = self.state.lock();
        let session = state
            .sessions
            .get_mut(&context.session)
            .ok_or_else(|| lifecycle_error("intent session was not opened"))?;
        if session.principal != context.principal
            || session.outbound.source_window() != context.source_window
        {
            return Err(lifecycle_error("mapped intent session identity changed"));
        }
        session.ready = true;
        Ok(())
    }

    fn session_closed(&self, context: &ProviderSessionContext, _reason: ProviderSessionEnd) {
        self.remove_session(context);
    }

    fn session_revoked(&self, context: &ProviderSessionContext) {
        self.remove_session(context);
    }
}

fn validate_invocation(
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
            "archetype" | "action" | "convention" | "payload" | "handler" | "behavior"
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
    let convention = optional_text(request, object, "convention", limits.maximum_text_bytes)?;
    if convention
        .as_deref()
        .is_some_and(|value| !value.starts_with("napplet:"))
    {
        return Err(invalid(
            request,
            "`convention` must use the napplet: namespace",
        ));
    }
    let handler_request = match object.get("handler").and_then(Value::as_str) {
        None | Some("default") => IntentHandlerRequest::Default,
        Some("choose") => IntentHandlerRequest::Choose,
        Some(value) if valid_text(value, limits.maximum_text_bytes) => {
            IntentHandlerRequest::Specific(Arc::from(value))
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
        convention,
        payload,
        handler_request,
        behavior,
    })
}

fn matching_handlers(
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

fn availability_locked(state: &IntentState, archetype: &str) -> IntentAvailability {
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

fn candidate_view(handler: &RegisteredHandler, is_default: bool) -> IntentCandidate {
    IntentCandidate {
        d_tag: Arc::from(handler.principal.d_tag()),
        title: handler.declaration.title.clone(),
        actions: handler.declaration.actions.iter().cloned().collect(),
        conventions: handler.declaration.conventions.iter().cloned().collect(),
        is_default,
    }
}

fn valid_declaration(declaration: &IntentHandlerDeclaration, limits: IntentProviderLimits) -> bool {
    valid_slug(&declaration.archetype)
        && declaration
            .title
            .as_deref()
            .is_none_or(|title| valid_text(title, limits.maximum_text_bytes))
        && !declaration.actions.is_empty()
        && declaration.actions.len() <= limits.maximum_actions_per_handler
        && declaration.actions.iter().all(|action| valid_slug(action))
        && declaration.conventions.len() <= limits.maximum_conventions_per_handler
        && declaration.conventions.iter().all(|convention| {
            valid_text(convention, limits.maximum_text_bytes) && convention.starts_with("napplet:")
        })
}

fn validate_limits(limits: IntentProviderLimits) -> Result<(), IntentProviderBuildError> {
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

fn exact_object<'a>(
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

fn required_text<'a>(
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

fn optional_text(
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

fn valid_text(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && !value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
}

fn valid_slug(value: &str) -> bool {
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

fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
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

fn lifecycle_error(reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(INTENT_DOMAIN),
        action: Arc::from("lifecycle"),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests;
