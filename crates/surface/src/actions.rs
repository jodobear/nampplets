use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    sync::Arc,
};

use nmp_native_runtime_core::{
    BoundedJson, Principal, ResourceClass, ResourceRefusal, ResourceTracker, SessionId,
};
use parking_lot::Mutex;
use serde_json::Value;
use thiserror::Error;

use crate::SurfaceDescriptor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionRouterLimits {
    pub maximum_handlers: usize,
    pub maximum_activity_facts: usize,
    pub maximum_payload_bytes: usize,
    pub maximum_result_bytes: usize,
}

impl Default for ActionRouterLimits {
    fn default() -> Self {
        Self {
            maximum_handlers: 128,
            maximum_activity_facts: 1_024,
            maximum_payload_bytes: 256 * 1024,
            maximum_result_bytes: 256 * 1024,
        }
    }
}

pub trait ActionHandler: Send + Sync + fmt::Debug {
    fn action_name(&self) -> &str;
    fn schema(&self) -> &str;
    fn validate(&self, payload: &Value) -> Result<(), Arc<str>>;
    fn handle(&self, request: ActionRequest) -> Result<ActionOutcome, Arc<str>>;
}

#[derive(Clone, Debug)]
pub struct ActionRequest {
    pub principal: Principal,
    pub session: SessionId,
    pub correlation_id: Option<Arc<str>>,
    pub payload: BoundedJson,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionOutcome {
    pub result: Option<BoundedJson>,
}

#[derive(Debug)]
pub struct ActionRouter {
    limits: ActionRouterLimits,
    resources: Arc<ResourceTracker>,
    handlers: BTreeMap<Arc<str>, Arc<dyn ActionHandler>>,
    activity: Mutex<VecDeque<RoutedActionFact>>,
}

impl ActionRouter {
    pub fn new(
        limits: ActionRouterLimits,
        resources: Arc<ResourceTracker>,
    ) -> Result<Self, ActionError> {
        if limits.maximum_handlers == 0
            || limits.maximum_activity_facts == 0
            || limits.maximum_payload_bytes == 0
            || limits.maximum_result_bytes == 0
        {
            return Err(ActionError::InvalidLimits);
        }
        Ok(Self {
            limits,
            resources,
            handlers: BTreeMap::new(),
            activity: Mutex::new(VecDeque::new()),
        })
    }

    pub fn register(&mut self, handler: Arc<dyn ActionHandler>) -> Result<(), ActionError> {
        if self.handlers.len() >= self.limits.maximum_handlers {
            return Err(ActionError::HandlerCapacity {
                capacity: self.limits.maximum_handlers,
            });
        }
        let action: Arc<str> = Arc::from(handler.action_name());
        if self.handlers.contains_key(&action) {
            return Err(ActionError::DuplicateHandler(action));
        }
        self.handlers.insert(action, handler);
        Ok(())
    }

    pub fn route(
        &self,
        descriptor: &SurfaceDescriptor,
        principal: Principal,
        session: SessionId,
        action: &str,
        correlation_id: Option<Arc<str>>,
        payload: &Value,
    ) -> Result<ActionOutcome, ActionError> {
        let Some(declaration) = descriptor
            .actions
            .iter()
            .find(|declaration| declaration.name == action)
        else {
            self.record(
                principal,
                session,
                Arc::from(action),
                ActionFactOutcome::Refused,
            );
            return Err(ActionError::Undeclared(Arc::from(action)));
        };
        let Some(handler) = self.handlers.get(action) else {
            self.record(
                principal,
                session,
                Arc::from(action),
                ActionFactOutcome::Refused,
            );
            return Err(ActionError::NoHandler(Arc::from(action)));
        };
        if handler.schema() != declaration.schema {
            self.record(
                principal,
                session,
                Arc::from(action),
                ActionFactOutcome::Refused,
            );
            return Err(ActionError::SchemaMismatch {
                declared: Arc::from(declaration.schema.as_str()),
                handler: Arc::from(handler.schema()),
            });
        }
        handler
            .validate(payload)
            .map_err(ActionError::InvalidPayload)?;
        let payload = BoundedJson::from_value(payload, self.limits.maximum_payload_bytes)
            .map_err(|error| ActionError::InvalidPayload(Arc::from(error.to_string())))?;
        let _lease = self
            .resources
            .admit(session, None, ResourceClass::Action)
            .map_err(ActionError::ResourceRefused)?;
        let request = ActionRequest {
            principal: principal.clone(),
            session,
            correlation_id,
            payload,
        };
        let result = handler.handle(request).map_err(ActionError::Handler)?;
        if result
            .result
            .as_ref()
            .is_some_and(|value| value.byte_len() > self.limits.maximum_result_bytes)
        {
            return Err(ActionError::ResultTooLarge);
        }
        self.record(
            principal,
            session,
            Arc::from(action),
            ActionFactOutcome::Completed,
        );
        Ok(result)
    }

    pub fn activity(&self) -> Vec<RoutedActionFact> {
        self.activity.lock().iter().cloned().collect()
    }

    fn record(
        &self,
        principal: Principal,
        session: SessionId,
        action: Arc<str>,
        outcome: ActionFactOutcome,
    ) {
        let mut activity = self.activity.lock();
        if activity.len() == self.limits.maximum_activity_facts {
            activity.pop_front();
        }
        activity.push_back(RoutedActionFact {
            principal,
            session,
            action,
            outcome,
        });
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedActionFact {
    pub principal: Principal,
    pub session: SessionId,
    pub action: Arc<str>,
    pub outcome: ActionFactOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionFactOutcome {
    Completed,
    Refused,
}

#[derive(Debug, Error)]
pub enum ActionError {
    #[error("action router limits must be finite and non-zero")]
    InvalidLimits,
    #[error("action handler capacity {capacity} is full")]
    HandlerCapacity { capacity: usize },
    #[error("duplicate action handler {0}")]
    DuplicateHandler(Arc<str>),
    #[error("surface did not declare action {0}")]
    Undeclared(Arc<str>),
    #[error("no host handler is registered for action {0}")]
    NoHandler(Arc<str>),
    #[error("declared action schema {declared} does not match handler schema {handler}")]
    SchemaMismatch {
        declared: Arc<str>,
        handler: Arc<str>,
    },
    #[error("invalid action payload: {0}")]
    InvalidPayload(Arc<str>),
    #[error("action handler failed: {0}")]
    Handler(Arc<str>),
    #[error("action result exceeded its finite byte limit")]
    ResultTooLarge,
    #[error(transparent)]
    ResourceRefused(#[from] ResourceRefusal),
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nmp_native_runtime_core::ResourceLimits;

    use crate::{ActionDescriptor, Fallback, SurfaceProfile};

    use super::*;

    #[derive(Debug)]
    struct ProfileOpen {
        called: AtomicUsize,
    }

    impl ActionHandler for ProfileOpen {
        fn action_name(&self) -> &str {
            "profile.open"
        }

        fn schema(&self) -> &str {
            "nostr.pubkey-ref/1"
        }

        fn validate(&self, payload: &Value) -> Result<(), Arc<str>> {
            if payload.get("pubkey").and_then(Value::as_str).is_some() {
                Ok(())
            } else {
                Err(Arc::from("missing pubkey"))
            }
        }

        fn handle(&self, _request: ActionRequest) -> Result<ActionOutcome, Arc<str>> {
            self.called.fetch_add(1, Ordering::AcqRel);
            Ok(ActionOutcome { result: None })
        }
    }

    fn descriptor() -> SurfaceDescriptor {
        SurfaceDescriptor {
            schema: "nmp.surface/1".to_owned(),
            profile: SurfaceProfile::Renderer,
            archetype: "feed".to_owned(),
            inputs: vec![],
            actions: vec![ActionDescriptor {
                name: "profile.open".to_owned(),
                schema: "nostr.pubkey-ref/1".to_owned(),
            }],
            fallback: Fallback::Legacy,
            presentation: None,
        }
    }

    fn principal() -> Principal {
        Principal::new("a".repeat(64), "feed", "b".repeat(64)).unwrap()
    }

    #[test]
    fn declared_action_routes_and_records_origin() {
        let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
        let handler = Arc::new(ProfileOpen {
            called: AtomicUsize::new(0),
        });
        let mut router = ActionRouter::new(ActionRouterLimits::default(), resources).unwrap();
        router.register(handler.clone()).unwrap();
        router
            .route(
                &descriptor(),
                principal(),
                SessionId(7),
                "profile.open",
                None,
                &serde_json::json!({"pubkey": "a"}),
            )
            .unwrap();
        assert_eq!(handler.called.load(Ordering::Acquire), 1);
        assert_eq!(router.activity()[0].principal, principal());
    }

    #[test]
    fn undeclared_action_never_runs_handler() {
        let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
        let handler = Arc::new(ProfileOpen {
            called: AtomicUsize::new(0),
        });
        let mut router = ActionRouter::new(ActionRouterLimits::default(), resources).unwrap();
        router.register(handler.clone()).unwrap();
        assert!(matches!(
            router.route(
                &descriptor(),
                principal(),
                SessionId(7),
                "system.exec",
                None,
                &serde_json::json!({}),
            ),
            Err(ActionError::Undeclared(_))
        ));
        assert_eq!(handler.called.load(Ordering::Acquire), 0);
    }
}
