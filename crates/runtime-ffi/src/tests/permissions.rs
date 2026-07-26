use super::*;

#[test]
fn good_morning_installs_with_exactly_its_own_signed_capability_profile() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);
    let artifact = controller
        .verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: D_TAG.to_owned(),
            },
        )
        .artifact
        .expect("published fixture verifies");
    assert!(
        artifact.requires().is_empty(),
        "the immutable manifest remains unchanged"
    );

    controller.install(Arc::clone(&artifact));
    let review = controller
        .permission_review(exact_coordinate(&artifact))
        .review
        .expect("the installed exact build has a permission review");
    assert!(
        review.capabilities.is_empty(),
        "no runtime special-casing survives install -- the manifest declares no `requires` \
         tags, so the review has nothing to decide"
    );
    assert!(review.launch_permitted);

    controller.launch(artifact, RuntimeExecutionProfile::Legacy);
    assert_eq!(
        controller.snapshot().sessions.len(),
        1,
        "an artifact with no required capabilities launches unconditionally"
    );
}

#[test]
fn permission_review_and_atomic_batch_are_exact_typed_and_restart_safe() {
    let temp = TempDir::new().unwrap();
    let runtime = controller(&temp);
    let coordinate = install_permission_fixture(&runtime);

    let initial = runtime.permission_review(coordinate.clone());
    assert!(initial.refusal.is_none());
    let initial = initial.review.unwrap();
    assert_eq!(initial.coordinate, coordinate);
    assert_eq!(initial.capabilities.len(), 2);
    assert_eq!(initial.capabilities[0].domain, "identity");
    assert_eq!(
        initial.capabilities[0].platform_availability,
        RuntimePermissionPlatformAvailability::Available
    );
    assert_eq!(
        initial.capabilities[0].sensitivity,
        RuntimePermissionSensitivity::Sensitive
    );
    assert_eq!(initial.capabilities[0].decision_options.len(), 4);
    assert_eq!(
        initial.capabilities[1].platform_availability,
        RuntimePermissionPlatformAvailability::Unknown {
            reason: "no provider metadata is registered for this capability on this runtime"
                .to_owned()
        }
    );

    let duplicate = runtime.apply_permission_decisions(RuntimePermissionDecisionBatch {
        coordinate: coordinate.clone(),
        decisions: vec![
            RuntimePermissionDecisionSelection {
                domain: "identity".to_owned(),
                decision: RuntimeGrantDecision::AllowExactBuild,
            },
            RuntimePermissionDecisionSelection {
                domain: "identity".to_owned(),
                decision: RuntimeGrantDecision::Denied,
            },
        ],
    });
    assert!(!duplicate.applied);
    assert_eq!(
        duplicate.refusal.unwrap().code,
        "duplicate-permission-domain"
    );

    let applied = runtime.apply_permission_decisions(RuntimePermissionDecisionBatch {
        coordinate: coordinate.clone(),
        decisions: vec![
            RuntimePermissionDecisionSelection {
                domain: "identity".to_owned(),
                decision: RuntimeGrantDecision::AllowExactBuild,
            },
            RuntimePermissionDecisionSelection {
                domain: "missing".to_owned(),
                decision: RuntimeGrantDecision::Denied,
            },
        ],
    });
    assert!(applied.applied);
    assert!(applied.refusal.is_none());
    let applied_review = applied.review.unwrap();
    assert!(applied_review.launch_permitted);
    assert_eq!(
        applied_review.capabilities[0].existing_decision,
        RuntimePermissionExistingDecision::AllowExactBuild
    );
    runtime.close();
    drop(runtime);

    let reopened = controller(&temp);
    let restored = reopened.permission_review(coordinate).review.unwrap();
    assert_eq!(restored.capabilities.len(), 2);
    assert_eq!(
        restored.capabilities[0].existing_decision,
        RuntimePermissionExistingDecision::AllowExactBuild
    );
    assert!(restored.launch_permitted);
}

#[test]
fn outbox_grant_survives_default_profile_restart() {
    let temp = TempDir::new().unwrap();
    let (event, author, digest) = signed_manifest_event(
        "restart-grant-test",
        b"<html>restart-grant</html>",
        vec![
            vec!["requires".to_owned(), "identity".to_owned()],
            vec!["requires".to_owned(), "inc".to_owned()],
            vec!["requires".to_owned(), "outbox".to_owned()],
        ],
    );
    let coordinate = ArtifactCoordinate::Named {
        author: author.clone(),
        d_tag: "restart-grant-test".to_owned(),
    };
    let runtime = RuntimeController::open(
        RuntimeConfig {
            runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
            nmp_store_path: None,
            artifact_cache_path: temp.path().join("artifacts").display().to_string(),
            ..RuntimeConfig::default()
        },
        Box::new(FixtureSource(BTreeMap::from([(
            digest.clone(),
            b"<html>restart-grant</html>".to_vec(),
        )]))),
    )
    .unwrap();
    let artifact = runtime
        .verify_artifact(event.clone(), coordinate.clone())
        .artifact
        .expect("locally signed fixture verifies");
    runtime.install(Arc::clone(&artifact));
    let coordinate = exact_coordinate(&artifact);
    let review = runtime
        .permission_review(coordinate.clone())
        .review
        .expect("installed napplet has a permission review");
    let update = runtime.apply_permission_decisions(RuntimePermissionDecisionBatch {
        coordinate: coordinate.clone(),
        decisions: review
            .capabilities
            .iter()
            .map(|capability| RuntimePermissionDecisionSelection {
                domain: capability.domain.clone(),
                decision: RuntimeGrantDecision::AllowExactBuild,
            })
            .collect(),
    });
    assert!(update.applied);
    assert!(update.review.unwrap().launch_permitted);
    runtime.close();
    drop(runtime);

    let reopened = RuntimeController::open(
        RuntimeConfig {
            runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
            nmp_store_path: None,
            artifact_cache_path: temp.path().join("artifacts").display().to_string(),
            ..RuntimeConfig::default()
        },
        Box::new(FixtureSource(BTreeMap::from([(
            digest,
            b"<html>restart-grant</html>".to_vec(),
        )]))),
    )
    .unwrap();
    let artifact = reopened
        .verify_artifact(
            event,
            ArtifactCoordinate::Named {
                author,
                d_tag: "restart-grant-test".to_owned(),
            },
        )
        .artifact
        .expect("locally signed fixture verifies after restart");
    reopened.install(Arc::clone(&artifact));
    let review = reopened
        .permission_review(coordinate)
        .review
        .expect("review restores after restart");
    for domain in ["identity", "inc", "outbox"] {
        let capability = review
            .capabilities
            .iter()
            .find(|capability| capability.domain == domain)
            .unwrap_or_else(|| panic!("missing required {domain} capability"));
        assert_eq!(
            capability.existing_decision,
            RuntimePermissionExistingDecision::AllowExactBuild
        );
    }
    assert!(review.launch_permitted);

    reopened.launch(artifact, RuntimeExecutionProfile::Legacy);
    let session = reopened.snapshot().sessions[0].clone();
    assert_eq!(
        session.domains,
        ["identity", "inc", "outbox", "shell"],
        "the restored exact-build grant must negotiate NAP-OUTBOX"
    );
    reopened.mapped_envelope(session.id, br#"{"type":"shell.ready"}"#.to_vec());
    assert_eq!(
        response_of_type(&reopened, "shell.init")["capabilities"]["domains"],
        serde_json::json!(["identity", "inc", "outbox", "shell"]),
        "the trusted shell must receive the same Rust-negotiated domain set"
    );
}
