//! Bounded provider tests for NAP-INC sessions, topics, and channels.
//!
//! Split across submodules only to stay under the repository's 600-line
//! file ceiling; the shared harness below is the single fixture both
//! halves use.

mod channels;
mod sessions;

use std::{
    collections::{BTreeSet, VecDeque},
    sync::atomic::{AtomicBool, Ordering},
};

use nmp_native_nap_bridge::{
    BridgeError, BridgeLimits, DispatchOutcome, InjectionPlan, MemoryActivitySink,
    ProviderPushObserver, ProviderRegistry, SessionContext, SourceWindowId,
};
use nmp_native_runtime_core::{
    ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, ResourceLimits, ResourceTracker,
    Sensitivity,
};

use super::*;

#[derive(Debug, Default)]
struct FakeActivity {
    facts: Mutex<Vec<IncActivity>>,
}

impl IncActivitySink for FakeActivity {
    fn record(&self, fact: IncActivity) {
        self.facts.lock().push(fact);
    }
}

#[derive(Debug)]
struct FakeNativeActions {
    capacity: usize,
    closed: AtomicBool,
    pending: Mutex<VecDeque<IncNativeAction>>,
    ended: Mutex<Vec<(IncNativeActionOrigin, IncNativeActionSessionEnd)>>,
}

impl FakeNativeActions {
    fn bounded(capacity: usize) -> Self {
        Self {
            capacity,
            closed: AtomicBool::new(false),
            pending: Mutex::new(VecDeque::with_capacity(capacity)),
            ended: Mutex::new(Vec::new()),
        }
    }

    fn drain(&self) -> Vec<IncNativeAction> {
        self.pending.lock().drain(..).collect()
    }
}

impl IncNativeActionSink for FakeNativeActions {
    fn try_enqueue(&self, action: IncNativeAction) -> Result<(), IncNativeActionSinkError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(IncNativeActionSinkError::Closed);
        }
        let mut pending = self.pending.lock();
        if pending.len() >= self.capacity {
            return Err(IncNativeActionSinkError::Backpressure);
        }
        pending.push_back(action);
        Ok(())
    }

    fn session_ended(&self, origin: &IncNativeActionOrigin, reason: IncNativeActionSessionEnd) {
        self.pending
            .lock()
            .retain(|action| action.origin != *origin);
        self.ended.lock().push((origin.clone(), reason));
    }
}

#[derive(Debug, Default)]
struct MutableAcl {
    deny_topics: AtomicBool,
    deny_channels: AtomicBool,
}

impl IncAcl for MutableAcl {
    fn allow_topic(&self, _request: TopicAclRequest<'_>) -> bool {
        !self.deny_topics.load(Ordering::Acquire)
    }

    fn allow_channel(&self, _request: ChannelAclRequest<'_>) -> bool {
        !self.deny_channels.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
struct FakeIds {
    ids: Mutex<VecDeque<Result<Arc<str>, ChannelIdError>>>,
}

impl FakeIds {
    fn new(ids: impl IntoIterator<Item = &'static str>) -> Self {
        Self {
            ids: Mutex::new(ids.into_iter().map(|id| Ok(Arc::from(id))).collect()),
        }
    }
}

impl ChannelIdGenerator for FakeIds {
    fn next_id(&self) -> Result<Arc<str>, ChannelIdError> {
        self.ids
            .lock()
            .pop_front()
            .unwrap_or(Err(ChannelIdError::Unavailable))
    }
}

struct OpenEndpoint {
    context: SessionContext,
    plan: InjectionPlan,
    observer: ProviderPushObserver,
}

struct Harness {
    provider: Arc<IncProvider>,
    acl: Arc<MutableAcl>,
    activity: Arc<FakeActivity>,
    grants: Arc<GrantLedger>,
    registry: ProviderRegistry,
    endpoints: BTreeMap<SessionId, OpenEndpoint>,
    now: u64,
}

impl Harness {
    fn new(limits: IncProviderLimits, push_count: usize) -> Self {
        Self::with_optional_native_actions(limits, push_count, None)
    }

    fn with_native_actions(
        limits: IncProviderLimits,
        push_count: usize,
        native_actions: Arc<dyn IncNativeActionSink>,
    ) -> Self {
        Self::with_optional_native_actions(limits, push_count, Some(native_actions))
    }

    fn with_optional_native_actions(
        limits: IncProviderLimits,
        push_count: usize,
        native_actions: Option<Arc<dyn IncNativeActionSink>>,
    ) -> Self {
        let acl = Arc::new(MutableAcl::default());
        let activity = Arc::new(FakeActivity::default());
        let acl_dyn: Arc<dyn IncAcl> = acl.clone();
        let activity_dyn: Arc<dyn IncActivitySink> = activity.clone();
        let ids: Arc<dyn ChannelIdGenerator> = Arc::new(FakeIds::new(["c-1", "c-2", "c-3", "c-4"]));
        let provider = Arc::new(match native_actions {
            Some(native_actions) => IncProvider::with_channel_ids_and_native_actions(
                acl_dyn,
                activity_dyn,
                ids,
                native_actions,
                limits,
            )
            .unwrap(),
            None => IncProvider::with_channel_ids(acl_dyn, activity_dyn, ids, limits).unwrap(),
        });
        let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
        let grants =
            Arc::new(GrantLedger::new(GrantLimits::default(), Arc::clone(&resources)).unwrap());
        let bridge_activity = Arc::new(MemoryActivitySink::bounded(256));
        let mut bridge_limits = BridgeLimits::default();
        bridge_limits.provider_pushes.maximum_pending_count = push_count;
        let mut registry = ProviderRegistry::new(
            bridge_limits,
            resources,
            Arc::clone(&grants),
            bridge_activity,
        )
        .unwrap();
        let registerable: Arc<dyn Provider> = provider.clone();
        registry.register(registerable).unwrap();
        Self {
            provider,
            acl,
            activity,
            grants,
            registry,
            endpoints: BTreeMap::new(),
            now: 0,
        }
    }

    fn open(&mut self, session: u64, d_tag: &str, hash: char) {
        let principal = Principal::new(
            format!("{:0<64}", session),
            d_tag,
            hash.to_string().repeat(64),
        )
        .unwrap();
        let domain = Capability::new(DOMAIN).unwrap();
        self.grants
            .set(
                principal.clone(),
                domain.clone(),
                Sensitivity::Sensitive,
                GrantDecision::AllowExactBuild,
            )
            .unwrap();
        let context = SessionContext {
            id: SessionId(session),
            principal,
            profile: ExecutionProfile::Legacy,
        };
        let plan = self
            .registry
            .negotiate(
                &context.principal,
                context.profile,
                &BTreeSet::from([domain]),
            )
            .unwrap();
        let observer = self
            .registry
            .open_session_bound(
                &context,
                &plan,
                SourceWindowId(session.saturating_add(100)),
                self.now,
            )
            .unwrap();
        self.registry.mark_session_ready(context.id).unwrap();
        self.endpoints.insert(
            context.id,
            OpenEndpoint {
                context,
                plan,
                observer,
            },
        );
    }

    fn dispatch(&mut self, session: u64, message: Value) -> Option<Value> {
        self.now = self.now.saturating_add(1);
        let endpoint = self.endpoints.get(&SessionId(session)).unwrap();
        match self
            .registry
            .dispatch(
                &endpoint.context,
                &endpoint.plan,
                &serde_json::to_vec(&message).unwrap(),
                self.now,
            )
            .unwrap()
        {
            DispatchOutcome::Handled(call) => call
                .response
                .map(|response| response.decode().expect("valid response")),
            DispatchOutcome::IgnoredUnknown => panic!("pinned INC action was ignored"),
        }
    }

    fn dispatch_error(&mut self, session: u64, message: Value) -> BridgeError {
        self.now = self.now.saturating_add(1);
        let endpoint = self.endpoints.get(&SessionId(session)).unwrap();
        self.registry
            .dispatch(
                &endpoint.context,
                &endpoint.plan,
                &serde_json::to_vec(&message).unwrap(),
                self.now,
            )
            .unwrap_err()
    }

    fn drain(&self, session: u64) -> Vec<Value> {
        self.endpoints[&SessionId(session)]
            .observer
            .drain(64)
            .unwrap()
            .pushes
            .into_iter()
            .map(|push| push.envelope.decode().unwrap())
            .collect()
    }

    fn open_channel(&mut self, opener: u64, target: &str, id: &str) -> String {
        self.dispatch(
            opener,
            json!({"type":"inc.channel.open","id":id,"target":target}),
        )
        .unwrap()
        .get("channelId")
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
    }
}

fn default_harness() -> Harness {
    Harness::new(IncProviderLimits::default(), 64)
}
