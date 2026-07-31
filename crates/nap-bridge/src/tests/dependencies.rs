use super::*;

#[test]
fn provider_is_absent_until_every_dependency_is_admitted() {
    let (mut registry, principal, grants, storage) = fixture(5);
    let identity = Capability::new("identity").unwrap();
    let relay = Capability::new("relay").unwrap();
    let lists = Capability::new("lists").unwrap();
    let groups = Capability::new("groups").unwrap();
    for domain in [&identity, &relay] {
        registry
            .register(Arc::new(EchoProvider {
                descriptor: ProviderDescriptor {
                    domain: domain.clone(),
                    protocol_versions: BTreeSet::from([Arc::from("1")]),
                    actions: BTreeSet::from([Arc::from("get")]),
                    sensitive: true,
                    dependencies: BTreeSet::new(),
                    platform_availability: ProviderPlatformAvailability::Available,
                },
                calls: Arc::new(AtomicUsize::new(0)),
            }))
            .unwrap();
    }
    registry
        .register(Arc::new(EchoProvider {
            descriptor: ProviderDescriptor {
                domain: lists.clone(),
                protocol_versions: BTreeSet::from([Arc::from("NAP-LISTS")]),
                actions: BTreeSet::from([Arc::from("supported")]),
                sensitive: true,
                dependencies: BTreeSet::from([identity.clone(), relay.clone()]),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            calls: Arc::new(AtomicUsize::new(0)),
        }))
        .unwrap();
    registry
        .register(Arc::new(EchoProvider {
            descriptor: ProviderDescriptor {
                domain: groups.clone(),
                protocol_versions: BTreeSet::from([Arc::from("NAP-GROUPS")]),
                actions: BTreeSet::from([Arc::from("list")]),
                sensitive: true,
                dependencies: BTreeSet::from([lists.clone()]),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            calls: Arc::new(AtomicUsize::new(0)),
        }))
        .unwrap();
    grants
        .set(
            principal.clone(),
            lists.clone(),
            Sensitivity::Sensitive,
            GrantDecision::AllowExactBuild,
        )
        .unwrap();

    let no_dependencies = registry
        .negotiate(&principal, ExecutionProfile::Legacy, &BTreeSet::new())
        .unwrap();
    assert!(!no_dependencies.exposes(&lists));
    assert_eq!(
        registry.revocation_scope(&identity),
        BTreeSet::from([identity.clone(), lists.clone(), groups.clone()])
    );
    assert_eq!(
        registry.revocation_scope(&relay),
        BTreeSet::from([relay.clone(), lists.clone(), groups])
    );

    for dependency in [&identity, &relay] {
        grants
            .set(
                principal.clone(),
                dependency.clone(),
                Sensitivity::Sensitive,
                GrantDecision::AllowExactBuild,
            )
            .unwrap();
    }
    let admitted = registry
        .negotiate(&principal, ExecutionProfile::Legacy, &BTreeSet::new())
        .unwrap();
    assert!(admitted.exposes(&lists));

    let context = SessionContext {
        id: SessionId(1),
        principal: principal.clone(),
        profile: ExecutionProfile::Legacy,
    };
    registry.open_session(&context, 0).unwrap();
    grants
        .set(
            principal.clone(),
            relay.clone(),
            Sensitivity::Sensitive,
            GrantDecision::Denied,
        )
        .unwrap();
    assert!(matches!(
        registry.dispatch(
            &context,
            &admitted,
            br#"{"type":"lists.supported"}"#,
            0,
        ),
        Err(BridgeError::CapabilityDenied { domain }) if domain == lists
    ));
    grants
        .set(
            principal.clone(),
            relay.clone(),
            Sensitivity::Sensitive,
            GrantDecision::AllowExactBuild,
        )
        .unwrap();

    let renderer = registry
        .negotiate(&principal, ExecutionProfile::Renderer, &BTreeSet::new())
        .unwrap();
    assert!(!renderer.exposes(&relay));
    assert!(!renderer.exposes(&lists));
    assert!(renderer.exposes(&identity));
    assert!(renderer.exposes(&storage));
}
