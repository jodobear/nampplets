use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{
    BridgeLimits, MemoryActivitySink, ProviderPushObserver, ProviderRegistry, SessionContext,
    SourceWindowId,
};
use nmp_native_provider_lists::{DOMAIN, ListsProvider};
use nmp_native_runtime_core::{
    Capability, ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, ResourceLimits,
    ResourceTracker, Sensitivity, SessionId,
};

use super::{dependency_provider::dependency_provider, principal};

/// Registers LISTS and its dependencies on a real bridge so every test uses
/// production grant, session-binding, and dispatch paths.
pub fn opened_session(provider: Arc<ListsProvider>) -> (ProviderRegistry, ProviderPushObserver) {
    let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
    let grants =
        Arc::new(GrantLedger::new(GrantLimits::default(), Arc::clone(&resources)).unwrap());
    let activity = Arc::new(MemoryActivitySink::bounded(32));
    let mut registry = ProviderRegistry::new(
        BridgeLimits::default(),
        resources,
        Arc::clone(&grants),
        activity,
    )
    .unwrap();
    for domain in ["identity", "relay"] {
        let capability = Capability::new(domain).unwrap();
        grants
            .set(
                principal(),
                capability.clone(),
                Sensitivity::Sensitive,
                GrantDecision::AllowExactBuild,
            )
            .unwrap();
        registry.register(dependency_provider(capability)).unwrap();
    }
    let lists = Capability::new(DOMAIN).unwrap();
    grants
        .set(
            principal(),
            lists.clone(),
            Sensitivity::Sensitive,
            GrantDecision::AllowExactBuild,
        )
        .unwrap();
    registry.register(provider).unwrap();
    let context = SessionContext {
        id: SessionId(7),
        principal: principal(),
        profile: ExecutionProfile::Legacy,
    };
    let plan = registry
        .negotiate(
            &context.principal,
            context.profile,
            &BTreeSet::from([lists]),
        )
        .unwrap();
    let observer = registry
        .open_session_bound(&context, &plan, SourceWindowId(11), 0)
        .unwrap();
    registry.mark_session_ready(context.id).unwrap();
    (registry, observer)
}
