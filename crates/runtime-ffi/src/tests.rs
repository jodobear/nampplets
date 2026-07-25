use std::{
    collections::BTreeMap,
    fs,
    time::{Duration, Instant},
};

use nmp_native_artifact::{AggregateVerifier, Nip5aPathTagsAggregate, Sha256Digest, VerifiedFile};
use nostr::{EventBuilder, Keys, Kind, Tag};
use serde_json::Value;
use tempfile::TempDir;

use super::*;

const EVENT: &[u8] =
    include_bytes!("../../../conformance/napplet-corpus/published/good-morning/event.json");
const INDEX: &[u8] =
    include_bytes!("../../../conformance/napplet-corpus/published/good-morning/index.html");
const AUTHOR: &str = "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5";
const D_TAG: &str = "good-morning";
const AGGREGATE_HASH: &str = "828a6df02afd56782ea20f805084acce65c53f7c37554948c1e0a64aa5a2b0a8";
const DIGEST: &str = "ffd35eea5c84d03cdda74c23e1bbb2c40500f503833503aa688036faa52f3808";

struct FixtureSource(BTreeMap<String, Vec<u8>>);

impl ArtifactSource for FixtureSource {
    fn fetch(&self, request: ArtifactFetchRequest) -> ArtifactFetchResponse {
        let bytes = self
            .0
            .get(&request.expected_sha256)
            .cloned()
            .unwrap_or_default();
        ArtifactFetchResponse::Body {
            source_url: request.candidate_urls[0].clone(),
            http_status: 200,
            bytes,
        }
    }
}

#[derive(Debug)]
struct FixtureAppearance;

impl NativeAppearanceSource for FixtureAppearance {
    fn current(&self) -> Option<NativeAppearanceSnapshot> {
        Some(NativeAppearanceSnapshot {
            dark: true,
            increased_contrast: false,
            reduced_transparency: false,
            accent_red: 88,
            accent_green: 166,
            accent_blue: 255,
        })
    }
}

#[derive(Debug)]
struct RecordingSettings {
    requests: Arc<Mutex<Vec<NativeSettingsRequest>>>,
}

impl NativeSettingsExecutor for RecordingSettings {
    fn try_open(&self, request: NativeSettingsRequest) -> NativeSettingsOpenResult {
        self.requests.lock().push(request);
        NativeSettingsOpenResult::Accepted
    }
}

#[derive(Debug)]
struct RecordingIncActions {
    requests: Arc<Mutex<Vec<NativeIncActionRequest>>>,
    ends: Arc<Mutex<Vec<NativeIncActionEnd>>>,
    result: NativeIncActionEnqueueResult,
}

impl NativeIncActionExecutor for RecordingIncActions {
    fn try_enqueue(&self, request: NativeIncActionRequest) -> NativeIncActionEnqueueResult {
        self.requests.lock().push(request);
        self.result
    }

    fn session_ended(&self, end: NativeIncActionEnd) {
        self.ends.lock().push(end);
    }
}

#[derive(Debug, Default)]
struct RecordingIntentActivation {
    requests: Mutex<Vec<NativeIntentActivationRequest>>,
}

impl NativeIntentActivationExecutor for RecordingIntentActivation {
    fn focus_or_launch(&self, handler: NativeIntentActivationRequest) {
        self.requests.lock().push(handler);
    }
}

fn controller(temp: &TempDir) -> Arc<RuntimeController> {
    RuntimeController::open(
        RuntimeConfig {
            runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
            nmp_store_path: None,
            artifact_cache_path: temp.path().join("artifacts").display().to_string(),
            ..RuntimeConfig::default()
        },
        Box::new(FixtureSource(BTreeMap::from([(
            DIGEST.to_owned(),
            INDEX.to_vec(),
        )]))),
    )
    .unwrap()
}

fn controller_with_native_capabilities(
    temp: &TempDir,
    requests: Arc<Mutex<Vec<NativeSettingsRequest>>>,
) -> Arc<RuntimeController> {
    RuntimeController::open_with_native_capabilities(
        RuntimeConfig {
            runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
            nmp_store_path: None,
            artifact_cache_path: temp.path().join("artifacts").display().to_string(),
            ..RuntimeConfig::default()
        },
        Box::new(FixtureSource(BTreeMap::from([(
            DIGEST.to_owned(),
            INDEX.to_vec(),
        )]))),
        Box::new(FixtureAppearance),
        Box::new(RecordingSettings { requests }),
    )
    .unwrap()
}

fn controller_with_all_native_capabilities(
    temp: &TempDir,
    requests: Arc<Mutex<Vec<NativeIncActionRequest>>>,
    ends: Arc<Mutex<Vec<NativeIncActionEnd>>>,
    result: NativeIncActionEnqueueResult,
) -> Arc<RuntimeController> {
    RuntimeController::open_with_all_native_capabilities(
        RuntimeConfig {
            runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
            nmp_store_path: None,
            artifact_cache_path: temp.path().join("artifacts").display().to_string(),
            ..RuntimeConfig::default()
        },
        Box::new(FixtureSource(BTreeMap::from([(
            DIGEST.to_owned(),
            INDEX.to_vec(),
        )]))),
        Box::new(FixtureAppearance),
        Box::new(RecordingSettings {
            requests: Arc::new(Mutex::new(Vec::new())),
        }),
        Box::new(RecordingIncActions {
            requests,
            ends,
            result,
        }),
        Box::new(RecordingIntentActivation::default()),
    )
    .unwrap()
}

fn workspace_definition(id: &str) -> RuntimeWorkspaceDefinition {
    RuntimeWorkspaceDefinition {
        schema_version: WORKSPACE_SCHEMA_VERSION,
        workspace_id: id.to_owned(),
        axis: RuntimeWorkspaceAxis::Horizontal,
        slots: vec![
            RuntimeWorkspaceSlot {
                slot_id: "feed".to_owned(),
                role: RuntimeWorkspaceRole::Feed,
                renderer: RuntimeWorkspaceRenderer::LegacyNapplet,
                handler_id: "good-morning".to_owned(),
                manifest_author: Some(AUTHOR.to_owned()),
                d_tag: Some("good-morning".to_owned()),
                aggregate_hash: Some("a".repeat(64)),
                binding_parameters_json: r#"{"window":{"limit":100}}"#.to_owned(),
                navigation_json: r#"{"selection":null}"#.to_owned(),
                visible: true,
                order: 0,
                size_points: 640,
                minimum_points: 320,
                maximum_points: 1_200,
            },
            RuntimeWorkspaceSlot {
                slot_id: "detail".to_owned(),
                role: RuntimeWorkspaceRole::Detail,
                renderer: RuntimeWorkspaceRenderer::Native,
                handler_id: "native-detail".to_owned(),
                manifest_author: None,
                d_tag: None,
                aggregate_hash: None,
                binding_parameters_json: "{}".to_owned(),
                navigation_json: "{}".to_owned(),
                visible: true,
                order: 1,
                size_points: 360,
                minimum_points: 240,
                maximum_points: 900,
            },
        ],
        focused_slot_id: Some("feed".to_owned()),
        activity_drawer_visible: true,
        preferences_json: r#"{"sidebar":"home"}"#.to_owned(),
        retained_receipt_ids: Vec::new(),
    }
}

fn exact_coordinate(artifact: &VerifiedArtifact) -> RuntimeExactBuildCoordinate {
    RuntimeExactBuildCoordinate {
        manifest_author: artifact.author(),
        d_tag: artifact.d_tag().expect("named fixture"),
        aggregate_hash: artifact.aggregate_hash(),
    }
}

fn install_permission_fixture(controller: &Arc<RuntimeController>) -> RuntimeExactBuildCoordinate {
    let artifact = controller
        .verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: "good-morning".to_owned(),
            },
        )
        .artifact
        .unwrap();
    let principal = artifact.principal.clone().unwrap();
    let executable: Arc<dyn ExecutableArtifact> = artifact.handle.clone();
    controller.app.dispatch(PlatformCommand::InstallVerified {
        build: InstalledBuild {
            principal,
            title: Arc::from("Good Morning Protocol"),
            manifest_metadata: BoundedJson::from_value(&serde_json::json!({"kind": 35129}), 1_024)
                .unwrap(),
            capability_requests: vec![
                CapabilityRequest {
                    capability: Capability::new("identity").unwrap(),
                    requirement: CapabilityRequirement::Required,
                },
                CapabilityRequest {
                    capability: Capability::new("missing").unwrap(),
                    requirement: CapabilityRequirement::Optional,
                },
            ],
        },
        artifact: executable,
    });
    exact_coordinate(&artifact)
}

fn install_and_launch(
    controller: &Arc<RuntimeController>,
    domains: &[&str],
) -> (Arc<VerifiedArtifact>, u64) {
    let artifact = controller
        .verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: "good-morning".to_owned(),
            },
        )
        .artifact
        .unwrap();
    controller.install(Arc::clone(&artifact));
    for domain in ["identity", "inc", "outbox"]
        .into_iter()
        .chain(domains.iter().copied())
    {
        controller.set_grant(
            Arc::clone(&artifact),
            domain.to_owned(),
            RuntimeSensitivity::Ordinary,
            RuntimeGrantDecision::AllowExactBuild,
        );
    }
    controller.launch(Arc::clone(&artifact), RuntimeExecutionProfile::Legacy);
    let session = controller.snapshot().sessions[0].id;
    controller.mapped_envelope(session, br#"{"type":"shell.ready"}"#.to_vec());
    (artifact, session)
}

/// Signs a minimal single-file named manifest with a fresh keypair, for
/// tests that need more than one distinct installable napplet (the
/// pinned `EVENT`/`AUTHOR` fixture above is one fixed real signed
/// event and can't be reused for that).
fn signed_manifest_event(
    d_tag: &str,
    content: &[u8],
    extra_tags: Vec<Vec<String>>,
) -> (Vec<u8>, String, String) {
    let digest = Sha256Digest::of(content);
    let aggregate = Nip5aPathTagsAggregate
        .compute(&[VerifiedFile {
            path: Arc::from("/index.html"),
            digest: digest.clone(),
            bytes: Arc::from(content),
        }])
        .unwrap();
    let mut tags = vec![
        vec!["d".to_owned(), d_tag.to_owned()],
        vec![
            "path".to_owned(),
            "/index.html".to_owned(),
            digest.as_str().to_owned(),
        ],
        vec![
            "x".to_owned(),
            aggregate.as_str().to_owned(),
            "aggregate".to_owned(),
        ],
        vec!["server".to_owned(), "https://blossom.example/".to_owned()],
    ];
    tags.extend(extra_tags);
    let keys = Keys::generate();
    let event = EventBuilder::new(Kind::Custom(35_129), "")
        .tags(
            tags.into_iter()
                .map(|tag| Tag::parse(tag).unwrap())
                .collect::<Vec<_>>(),
        )
        .sign_with_keys(&keys)
        .unwrap();
    (
        serde_json::to_vec(&event).unwrap(),
        event.pubkey.to_hex(),
        digest.as_str().to_owned(),
    )
}

fn response_of_type(controller: &RuntimeController, expected: &str) -> Value {
    controller
        .app
        .events_after(0)
        .events
        .into_iter()
        .rev()
        .find_map(|event| match event.event {
            PlatformEvent::EnvelopeHandled {
                response: Some(response),
                ..
            } if response.decode().ok()?.get("type")? == expected => response.decode().ok(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing `{expected}` response"))
}

#[test]
fn signed_artifact_crosses_only_as_sealed_handle_and_exact_reads() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);
    let verified = controller.verify_artifact(
        EVENT.to_vec(),
        ArtifactCoordinate::Named {
            author: AUTHOR.to_owned(),
            d_tag: "good-morning".to_owned(),
        },
    );
    let artifact = verified.artifact.expect("published fixture verifies");
    assert!(verified.refusal.is_none());
    assert!(artifact.requires().is_empty());
    controller.install(Arc::clone(&artifact));
    controller.set_grant(
        Arc::clone(&artifact),
        "shell".to_owned(),
        RuntimeSensitivity::Ordinary,
        RuntimeGrantDecision::AllowExactBuild,
    );
    for domain in ["identity", "inc", "outbox"] {
        controller.set_grant(
            Arc::clone(&artifact),
            domain.to_owned(),
            RuntimeSensitivity::Sensitive,
            RuntimeGrantDecision::AllowExactBuild,
        );
    }
    controller.launch(artifact, RuntimeExecutionProfile::Legacy);
    let runtime_snapshot = controller.snapshot();
    assert_eq!(
        runtime_snapshot.sessions[0].domains,
        ["identity", "inc", "outbox", "shell"]
    );
    let session = runtime_snapshot.sessions[0].id;
    controller.mapped_envelope(session, br#"{"type":"shell.ready"}"#.to_vec());
    controller.mapped_envelope(
        session,
        br#"{"type":"identity.getPublicKey","id":"identity-1"}"#.to_vec(),
    );
    let identity_response = controller
        .app
        .events_after(0)
        .events
        .into_iter()
        .find_map(|event| match event.event {
            PlatformEvent::EnvelopeHandled {
                response: Some(response),
                ..
            } if response.decode().ok()?.get("type")? == "identity.getPublicKey.result" => {
                response.decode().ok()
            }
            _ => None,
        })
        .expect("registered identity provider responds through the runtime");
    assert_eq!(identity_response["id"], "identity-1");
    assert_eq!(identity_response["pubkey"], "");
    controller.mapped_envelope(
        session,
        br#"{"type":"inc.subscribe","id":"inc-1","topic":"profile:open"}"#.to_vec(),
    );
    let inc_response = controller
        .app
        .events_after(0)
        .events
        .into_iter()
        .find_map(|event| match event.event {
            PlatformEvent::EnvelopeHandled {
                response: Some(response),
                ..
            } if response.decode().ok()?.get("type")? == "inc.subscribe.result" => {
                response.decode().ok()
            }
            _ => None,
        })
        .expect("registered INC provider responds through the runtime");
    assert_eq!(inc_response["id"], "inc-1");
    match controller.read_verified(session, "/index.html".to_owned(), 1_024 * 1_024) {
        VerifiedRead::Bytes { bytes, .. } => assert_eq!(bytes, INDEX),
        VerifiedRead::Refused { refusal } => panic!("{refusal:?}"),
    }
    assert!(matches!(
        controller.read_verified(session, "/../secret".to_owned(), 1_024),
        VerifiedRead::Refused { .. }
    ));
    controller.close();
    assert!(controller.snapshot().closed);
    assert!(fs::metadata(temp.path().join("runtime.sqlite3")).is_ok());
}

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

#[test]
fn installed_library_projects_filter_lifecycle_workspace_and_uninstall() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);
    let artifact = controller
        .verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: "good-morning".to_owned(),
            },
        )
        .artifact
        .expect("fixture verifies");
    let coordinate = exact_coordinate(&artifact);

    assert_eq!(controller.snapshot().installed_library.total_installed, 0);
    controller.install(Arc::clone(&artifact));
    let installed = controller.snapshot().installed_library;
    assert_eq!(installed.query, "");
    assert_eq!(installed.total_installed, 1);
    assert_eq!(installed.builds.len(), 1);
    assert_eq!(installed.builds[0].coordinate, coordinate);
    assert_eq!(
        installed.builds[0].availability,
        RuntimeInstalledBuildAvailability::SealedExactBytesReady
    );
    assert!(installed.builds[0].active_session_ids.is_empty());
    assert!(installed.builds[0].assigned_workspace_ids.is_empty());
    assert!(serde_json::from_str::<Value>(&installed.builds[0].manifest_metadata_json).is_ok());

    controller.set_library_filter("no-match".to_owned());
    let filtered = controller.snapshot().installed_library;
    assert_eq!(filtered.query, "no-match");
    assert_eq!(filtered.total_installed, 1);
    assert!(filtered.builds.is_empty());
    controller.set_library_filter("GOOD-MORNING".to_owned());
    assert_eq!(controller.snapshot().installed_library.builds.len(), 1);

    assert!(
        controller
            .save_workspace(workspace_definition("library"))
            .accepted
    );
    controller.assign_build_to_workspace("library".to_owned(), coordinate.clone());
    assert_eq!(
        controller.snapshot().installed_library.builds[0].assigned_workspace_ids,
        ["library"]
    );

    controller.launch(Arc::clone(&artifact), RuntimeExecutionProfile::Legacy);
    assert_eq!(
        controller.snapshot().sessions.len(),
        1,
        "an artifact with no required capabilities launches unconditionally"
    );
    let session = controller.snapshot().installed_library.builds[0].active_session_ids[0];
    controller.suspend(session);
    assert_eq!(controller.snapshot().sessions[0].state, "suspended");
    controller.resume(session);
    assert_eq!(controller.snapshot().sessions[0].state, "running");

    controller.clear_build_from_workspace("library".to_owned(), coordinate.clone());
    assert!(
        controller.snapshot().installed_library.builds[0]
            .assigned_workspace_ids
            .is_empty()
    );

    controller.uninstall_build(coordinate.clone());
    let uninstalled = controller.snapshot();
    assert_eq!(uninstalled.installed_library.total_installed, 0);
    assert!(uninstalled.installed_library.builds.is_empty());
    assert!(uninstalled.sessions.is_empty());
    assert!(uninstalled.workspaces.iter().any(|workspace| {
        workspace.workspace_id == "library" && workspace.retained_receipt_ids.is_empty()
    }));
    assert!(
        !controller.artifacts.lock().contains_key(
            &Principal::new(
                coordinate.manifest_author,
                coordinate.d_tag,
                coordinate.aggregate_hash
            )
            .unwrap()
        ),
        "the boundary must release its live verifier handle after kernel-confirmed uninstall"
    );
}

#[test]
fn installed_artifact_reacquisition_reuses_the_live_exact_handle() {
    let temp = TempDir::new().unwrap();
    let runtime = controller(&temp);
    let artifact = runtime
        .verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: "good-morning".to_owned(),
            },
        )
        .artifact
        .expect("fixture verifies");
    let coordinate = exact_coordinate(&artifact);
    runtime.install(artifact);
    runtime.set_library_filter("does-not-match".to_owned());

    let reopened = runtime.reacquire_installed_artifact(coordinate);
    assert!(reopened.failure.is_none());
    let confirmation = reopened.confirmation.expect("exact confirmation");
    let event: Value = serde_json::from_slice(EVENT).unwrap();
    assert_eq!(
        confirmation.event_id,
        event["id"].as_str().expect("fixture event id")
    );
    assert_eq!(confirmation.manifest_author, AUTHOR);
    assert_eq!(confirmation.d_tag.as_deref(), Some("good-morning"));
    assert_eq!(confirmation.aggregate_hash, AGGREGATE_HASH);
    assert_eq!(
        reopened
            .artifact
            .expect("opaque artifact")
            .handle
            .read_verified(nmp_native_artifact::INDEX_PATH, INDEX.len())
            .unwrap(),
        INDEX
    );
}

#[test]
fn persisted_install_reopens_offline_from_the_sealed_cache_after_restart() {
    let temp = TempDir::new().unwrap();
    let runtime = controller(&temp);
    let artifact = runtime
        .verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: "good-morning".to_owned(),
            },
        )
        .artifact
        .expect("fixture verifies");
    runtime.install(Arc::clone(&artifact));
    let coordinate = exact_coordinate(&artifact);
    runtime.close();
    drop(runtime);

    let reopened = controller(&temp);
    assert_eq!(
        reopened.snapshot().installed_library.builds[0].availability,
        RuntimeInstalledBuildAvailability::MetadataOnly
    );
    let result = reopened.reacquire_installed_artifact(coordinate);
    assert!(result.failure.is_none());
    let confirmation = result.confirmation.expect("exact confirmation");
    assert_eq!(confirmation.manifest_author, AUTHOR);
    assert_eq!(confirmation.d_tag.as_deref(), Some("good-morning"));
    assert_eq!(confirmation.aggregate_hash, AGGREGATE_HASH);
    assert_eq!(
        result
            .artifact
            .expect("opaque artifact")
            .handle
            .read_verified(nmp_native_artifact::INDEX_PATH, INDEX.len())
            .unwrap(),
        INDEX
    );
    // No network fetch happened -- this is offline reopen from the
    // sealed cache -- and the runtime's own live handle map now has the
    // reconstructed artifact attached, same as after a fresh install.
    assert_eq!(
        reopened.snapshot().installed_library.builds[0].availability,
        RuntimeInstalledBuildAvailability::SealedExactBytesReady
    );
}

#[test]
fn reacquire_refuses_a_legacy_install_with_no_retained_signed_event() {
    let temp = TempDir::new().unwrap();
    let runtime = controller(&temp);
    let artifact = runtime
        .verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: "good-morning".to_owned(),
            },
        )
        .artifact
        .expect("fixture verifies");
    runtime.install(Arc::clone(&artifact));
    let coordinate = exact_coordinate(&artifact);
    // Simulate a build installed before offline reopen was supported:
    // its persisted manifest metadata has no retained signed event.
    let mut legacy_build = runtime
        .runtime_store
        .installed_builds()
        .unwrap()
        .into_iter()
        .find(|build| build.principal.manifest_author() == AUTHOR)
        .expect("installed build");
    legacy_build.manifest_metadata =
        BoundedJson::from_value(&serde_json::json!({"event_id": "legacy"}), 4_096).unwrap();
    runtime.runtime_store.install(&legacy_build).unwrap();
    runtime.close();
    drop(runtime);

    let reopened = controller(&temp);
    let result = reopened.reacquire_installed_artifact(coordinate);
    assert!(result.artifact.is_none());
    assert_eq!(
        result.failure.expect("typed refusal").code,
        "installed-manifest-event-unavailable"
    );
}

#[test]
fn installed_artifact_reattach_refuses_signed_event_drift() {
    let temp = TempDir::new().unwrap();
    let runtime = controller(&temp);
    let artifact = runtime
        .verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: "good-morning".to_owned(),
            },
        )
        .artifact
        .expect("fixture verifies");
    runtime.install(Arc::clone(&artifact));
    let mut installed = runtime.app.snapshot().library.builds[0].build.clone();
    installed.manifest_metadata = BoundedJson::from_value(
        &serde_json::json!({
            "event_id": "0".repeat(64),
            "kind": 35_129,
            "mode": "single-file",
            "paths": 1,
        }),
        1_024,
    )
    .unwrap();

    let failure = runtime
        .verified_installed_artifact(&installed, Arc::clone(&artifact.handle))
        .expect_err("a different signed event must not inherit the persisted install");
    assert_eq!(failure.code, "installed-artifact-mismatch");
}

#[test]
fn installed_library_restores_metadata_only_and_refuses_invalid_inputs() {
    let temp = TempDir::new().unwrap();
    let runtime = controller(&temp);
    let artifact = runtime
        .verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: "good-morning".to_owned(),
            },
        )
        .artifact
        .expect("fixture verifies");
    runtime.install(artifact);
    runtime.close();
    drop(runtime);

    let reopened = controller(&temp);
    let restored = reopened.snapshot().installed_library;
    assert_eq!(restored.total_installed, 1);
    assert_eq!(restored.builds.len(), 1);
    assert_eq!(
        restored.builds[0].availability,
        RuntimeInstalledBuildAvailability::MetadataOnly
    );
    assert!(restored.builds[0].active_session_ids.is_empty());

    reopened.uninstall_build(RuntimeExactBuildCoordinate {
        manifest_author: AUTHOR.to_ascii_uppercase(),
        d_tag: "good-morning".to_owned(),
        aggregate_hash: restored.builds[0].coordinate.aggregate_hash.clone(),
    });
    let snapshot = reopened.snapshot();
    assert_eq!(snapshot.installed_library.total_installed, 1);
    assert_eq!(
        snapshot.boundary_refusals.last().unwrap().code,
        "invalid-exact-build-coordinate"
    );

    reopened.assign_build_to_workspace("\n".to_owned(), restored.builds[0].coordinate.clone());
    assert_eq!(
        reopened.snapshot().boundary_refusals.last().unwrap().code,
        "invalid-workspace-assignment"
    );

    reopened.set_library_filter("x".repeat(AppLimits::default().maximum_library_query_bytes + 1));
    let refused = reopened.snapshot();
    assert_eq!(refused.installed_library.query, "");
    assert_eq!(refused.recent_errors.last().unwrap().code, "capacity");
}

#[test]
fn malformed_manifest_is_a_semantic_refusal() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);
    let result = controller.verify_artifact(
        b"{}".to_vec(),
        ArtifactCoordinate::Named {
            author: "0".repeat(64),
            d_tag: "fixture".to_owned(),
        },
    );
    assert!(result.artifact.is_none());
    assert_eq!(result.refusal.unwrap().code, "artifact-verification");
}

#[test]
fn envelope_response_is_projected_as_exact_machine_readable_json() {
    let response =
        BoundedJson::from_raw(r#"{"type":"shell.init","capabilities":{}}"#, 1_024).unwrap();
    let event = PlatformEvent::EnvelopeHandled {
        session: SessionId(7),
        operation: None,
        response: Some(response.clone()),
    };

    let projected = project_event(11, &event);
    assert_eq!(projected.kind, "envelope-handled");
    assert_eq!(projected.session_id, Some(7));
    assert_eq!(projected.response_json.as_deref(), Some(response.as_str()));
}

#[test]
fn provider_push_is_projected_as_exact_machine_readable_json() {
    let envelope =
        BoundedJson::from_raw(r#"{"type":"identity.changed","pubkey":"abc"}"#, 1_024).unwrap();
    let event = PlatformEvent::ProviderPush {
        session: SessionId(9),
        source_window: nmp_native_nap_bridge::SourceWindowId(3),
        provider_sequence: 4,
        domain: Capability::new("identity").unwrap(),
        envelope: envelope.clone(),
    };

    let projected = project_event(12, &event);
    assert_eq!(projected.kind, "provider-push");
    assert_eq!(projected.session_id, Some(9));
    assert_eq!(projected.response_json.as_deref(), Some(envelope.as_str()));
}

#[test]
fn native_capabilities_are_absent_unless_supplied() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);
    let (_, session) = install_and_launch(&controller, &["theme", "config"]);
    let snapshot = controller.snapshot();
    let domains = &snapshot
        .sessions
        .iter()
        .find(|candidate| candidate.id == session)
        .unwrap()
        .domains;
    assert!(!domains.iter().any(|domain| domain == "theme"));
    assert!(!domains.iter().any(|domain| domain == "config"));
}

#[test]
fn native_theme_and_settings_cross_the_exact_build_boundary() {
    let temp = TempDir::new().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let controller = controller_with_native_capabilities(&temp, Arc::clone(&requests));
    let (_, session) = install_and_launch(&controller, &["theme", "config"]);
    let domains = &controller.snapshot().sessions[0].domains;
    assert!(domains.iter().any(|domain| domain == "theme"));
    assert!(domains.iter().any(|domain| domain == "config"));

    controller.mapped_envelope(session, br#"{"type":"theme.get","id":"theme-1"}"#.to_vec());
    assert_eq!(
        response_of_type(&controller, "theme.get.result")["theme"]["colors"],
        serde_json::json!({
            "background": "#1c1c1e",
            "text": "#ffffff",
            "primary": "#58a6ff"
        })
    );
    let changed = controller.update_appearance(NativeAppearanceSnapshot {
        dark: false,
        increased_contrast: true,
        reduced_transparency: true,
        accent_red: 0,
        accent_green: 102,
        accent_blue: 204,
    });
    assert!(changed.accepted);
    assert_eq!(changed.attempted, 1);
    assert_eq!(changed.delivered, 1);

    let schema = serde_json::json!({
        "$version": 1,
        "type": "object",
        "properties": {
            "mode": {
                "type": "string",
                "enum": ["quiet", "loud"],
                "default": "quiet",
                "x-napplet-section": "appearance"
            },
            "enabled": {"type": "boolean", "default": true}
        },
        "additionalProperties": false
    });
    controller.mapped_envelope(
        session,
        serde_json::to_vec(&serde_json::json!({
            "type": "config.registerSchema",
            "id": "schema-1",
            "schema": schema,
            "version": 1
        }))
        .unwrap(),
    );
    assert_eq!(
        response_of_type(&controller, "config.registerSchema.result")["ok"],
        true
    );
    controller.mapped_envelope(session, br#"{"type":"config.subscribe"}"#.to_vec());
    controller.mapped_envelope(
        session,
        br#"{"type":"config.openSettings","section":"appearance"}"#.to_vec(),
    );
    let request = requests.lock().pop().expect("native settings request");
    assert_eq!(request.manifest_author, AUTHOR);
    assert_eq!(request.d_tag, "good-morning");
    assert_eq!(request.session_id, session);
    assert_eq!(request.section.as_deref(), Some("appearance"));
    assert!(request.schema_json.len() <= 192 * 1_024);
    assert!(request.values_json.len() <= 192 * 1_024);

    let commit = controller.commit_config_values(NativeConfigCommit {
        manifest_author: request.manifest_author,
        d_tag: request.d_tag,
        aggregate_hash: request.aggregate_hash,
        session_id: request.session_id,
        values_json: r#"{"enabled":false,"mode":"loud"}"#.to_owned(),
    });
    assert!(commit.accepted);
    assert_eq!(commit.attempted, 1);
    assert_eq!(commit.delivered, 1);
    controller.mapped_envelope(session, br#"{"type":"config.get","id":"get-1"}"#.to_vec());
    assert_eq!(
        response_of_type(&controller, "config.values")["values"],
        serde_json::json!({"enabled": false, "mode": "loud"})
    );

    controller.stop(session);
    let refused = controller.commit_config_values(NativeConfigCommit {
        manifest_author: AUTHOR.to_owned(),
        d_tag: "good-morning".to_owned(),
        aggregate_hash: "828a6df02afd56782ea20f805084acce65c53f7c37554948c1e0a64aa5a2b0a8"
            .to_owned(),
        session_id: session,
        values_json: r#"{"enabled":true,"mode":"quiet"}"#.to_owned(),
    });
    assert!(!refused.accepted);
    assert_eq!(refused.refusal.unwrap().code, "settings-session-closed");

    controller.close();
    drop(controller);
    let reopened = controller_with_native_capabilities(&temp, Arc::new(Mutex::new(Vec::new())));
    let (_, reopened_session) = install_and_launch(&reopened, &["config"]);
    reopened.mapped_envelope(
        reopened_session,
        br#"{"type":"config.get","id":"get-after-restart"}"#.to_vec(),
    );
    assert_eq!(
        response_of_type(&reopened, "config.values")["values"],
        serde_json::json!({"enabled": false, "mode": "loud"})
    );
}

#[test]
fn typed_workspace_round_trips_only_through_rust_owned_storage() {
    let temp = TempDir::new().unwrap();
    let runtime = controller(&temp);
    let expected = workspace_definition("primary");
    let saved = runtime.save_workspace(expected.clone());
    assert!(saved.accepted);
    assert_eq!(saved.workspace, Some(expected.clone()));
    assert_eq!(
        runtime.snapshot().workspaces,
        std::slice::from_ref(&expected)
    );
    runtime.close();
    drop(runtime);

    let reopened = controller(&temp);
    assert!(reopened.snapshot().workspaces.is_empty());
    let restored = reopened.restore_workspaces();
    assert!(restored.accepted);
    assert_eq!(restored.workspaces, std::slice::from_ref(&expected));
    assert_eq!(reopened.snapshot().workspaces, [expected]);
}

#[test]
fn workspace_validation_refuses_unknown_duplicate_and_oversized_input() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);

    let mut unknown = workspace_definition("unknown");
    unknown.schema_version = WORKSPACE_SCHEMA_VERSION + 1;
    let refusal = controller.save_workspace(unknown).refusal.unwrap();
    assert_eq!(refusal.code, "invalid-workspace");

    let mut duplicate = workspace_definition("duplicate");
    duplicate.slots[1].slot_id = duplicate.slots[0].slot_id.clone();
    let refusal = controller.save_workspace(duplicate).refusal.unwrap();
    assert_eq!(refusal.code, "invalid-workspace");
    assert!(refusal.detail.contains("duplicate workspace slot id"));

    let mut oversized = workspace_definition("oversized");
    oversized.preferences_json = format!(
        r#"{{"value":"{}"}}"#,
        "x".repeat(MAXIMUM_WORKSPACE_FIELD_BYTES)
    );
    let refusal = controller.save_workspace(oversized).refusal.unwrap();
    assert_eq!(refusal.code, "invalid-workspace");
    assert!(refusal.detail.contains("maximum"));
    assert!(controller.snapshot().workspaces.is_empty());
}

#[test]
fn workspace_restore_is_all_or_nothing_for_corrupt_or_future_rows() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);
    let valid = workspace_record_from_ffi(workspace_definition("a-valid")).unwrap();
    controller.runtime_store.save_workspace(&valid).unwrap();

    let mut future_value: serde_json::Value =
        serde_json::from_str(valid.definition.as_str()).unwrap();
    future_value["schema_version"] = serde_json::json!(WORKSPACE_SCHEMA_VERSION.saturating_add(1));
    controller
        .runtime_store
        .save_workspace(&WorkspaceRecord {
            id: Arc::from("z-future"),
            definition: BoundedJson::from_value(&future_value, MAXIMUM_WORKSPACE_JSON_BYTES)
                .unwrap(),
            retained_receipts: Vec::new(),
        })
        .unwrap();

    let restored = controller.restore_workspaces();
    assert!(!restored.accepted);
    assert!(restored.workspaces.is_empty());
    assert_eq!(restored.refusal.unwrap().code, "invalid-workspace");
    assert!(controller.snapshot().workspaces.is_empty());
}

#[test]
fn local_account_lifecycle_is_explicit_typed_and_stale_safe() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);

    let invalid = controller.register_local_account("not-a-secret".to_owned());
    assert!(!invalid.accepted);
    assert_eq!(
        invalid.failure,
        Some(RuntimeAccountFailure::InvalidSecretKey)
    );

    let registered = controller.register_local_account(format!("{:064x}", 7_u8));
    assert!(registered.accepted);
    let first = registered.handle.unwrap();
    assert_eq!(first.kind, RuntimeAccountKind::LocalSigner);
    assert_eq!(
        registered.snapshot.unwrap().active_public_key,
        None,
        "registration must not silently switch identity"
    );

    let activated = controller.activate_local_account(first.clone());
    assert!(activated.accepted);
    assert_eq!(
        activated.snapshot.unwrap().active_public_key.as_deref(),
        Some(first.public_key.as_str())
    );

    let replacement = controller
        .register_local_account(format!("{:064x}", 7_u8))
        .handle
        .unwrap();
    assert_ne!(first.installation_id, replacement.installation_id);
    let stale = controller.remove_local_account(first);
    assert!(!stale.accepted);
    assert_eq!(
        stale.failure,
        Some(RuntimeAccountFailure::StaleInstallation)
    );

    let activated = controller.activate_local_account(replacement.clone());
    assert!(activated.accepted);
    assert_eq!(
        controller
            .logout_local_account()
            .snapshot
            .unwrap()
            .active_public_key,
        None
    );
    let removed = controller.remove_local_account(replacement);
    assert!(removed.accepted);
    assert!(removed.snapshot.unwrap().local_accounts.is_empty());

    controller.close();
    assert_eq!(
        controller.account_snapshot().failure,
        Some(RuntimeAccountFailure::Closed)
    );
}

#[test]
fn read_only_account_lifecycle_is_keyless_typed_and_explicit() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);
    let npub = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6";

    let registered = controller.register_read_only_account(npub.to_owned());
    assert!(registered.accepted);
    let handle = registered.handle.unwrap();
    assert_eq!(handle.kind, RuntimeAccountKind::ReadOnly);
    assert_eq!(handle.public_key.len(), 64);
    assert_eq!(
        registered.snapshot.unwrap().active_public_key,
        None,
        "read-only registration must not silently switch identity"
    );

    let activated = controller.activate_local_account(handle.clone());
    assert!(activated.accepted);
    assert_eq!(
        activated.snapshot.unwrap().active_public_key.as_deref(),
        Some(handle.public_key.as_str())
    );
    assert_eq!(
        controller
            .logout_local_account()
            .snapshot
            .unwrap()
            .active_public_key,
        None
    );
    assert!(controller.remove_local_account(handle).accepted);
    assert!(
        controller
            .account_snapshot()
            .snapshot
            .unwrap()
            .local_accounts
            .is_empty()
    );

    let nip05 = controller.register_read_only_account("pablo@example.com".to_owned());
    assert_eq!(
        nip05.failure,
        Some(RuntimeAccountFailure::Nip05ResolutionUnavailable)
    );
    let invalid = controller.register_read_only_account("not-a-key".to_owned());
    assert_eq!(
        invalid.failure,
        Some(RuntimeAccountFailure::InvalidPublicKey)
    );
}

#[test]
fn native_inc_actions_cross_ffi_with_trusted_origin_and_teardown() {
    let temp = TempDir::new().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let ends = Arc::new(Mutex::new(Vec::new()));
    let controller = controller_with_all_native_capabilities(
        &temp,
        Arc::clone(&requests),
        Arc::clone(&ends),
        NativeIncActionEnqueueResult::Accepted,
    );
    let (_, session) = install_and_launch(&controller, &["inc"]);
    controller.mapped_envelope(
        session,
        serde_json::to_vec(&serde_json::json!({
            "type": "inc.emit",
            "topic": "profile:open",
            "payload": {"pubkey": AUTHOR}
        }))
        .unwrap(),
    );
    let request = requests.lock().pop().expect("native action request");
    assert_eq!(request.manifest_author, AUTHOR);
    assert_eq!(request.d_tag, "good-morning");
    assert_eq!(request.session_id, session);
    assert_eq!(request.source_window_id, session);
    assert_eq!(request.kind, "profile-open");
    assert_eq!(
        serde_json::from_str::<Value>(&request.payload_json).unwrap(),
        serde_json::json!({"pubkey": AUTHOR})
    );

    controller.stop(session);
    let end = ends.lock().pop().expect("session teardown callback");
    assert_eq!(end.session_id, session);
    assert!(end.reason.starts_with("closed-"));
}

#[test]
fn native_inc_action_backpressure_is_an_exact_provider_refusal() {
    let temp = TempDir::new().unwrap();
    let controller = controller_with_all_native_capabilities(
        &temp,
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        NativeIncActionEnqueueResult::Backpressure,
    );
    let (_, session) = install_and_launch(&controller, &["inc"]);
    controller.mapped_envelope(
        session,
        serde_json::to_vec(&serde_json::json!({
            "type": "inc.emit",
            "topic": "profile:open",
            "payload": {"pubkey": AUTHOR}
        }))
        .unwrap(),
    );
    let error = controller
        .snapshot()
        .recent_errors
        .into_iter()
        .last()
        .expect("provider refusal fact");
    assert_eq!(error.code, "bridge");
    assert!(
        error.detail.contains("native action capacity is full"),
        "unexpected refusal detail: {}",
        error.detail
    );
}

#[test]
fn catalog_coordinates_are_parsed_only_at_the_rust_boundary() {
    let author = "a".repeat(64);
    let event_id = "b".repeat(64);
    assert!(matches!(
        parse_catalog_coordinate(&format!("35129:{author}:good-morning")).unwrap(),
        ManifestCoordinate::Named { .. }
    ));
    assert!(matches!(
        parse_catalog_coordinate(&format!("15129:{author}")).unwrap(),
        ManifestCoordinate::Root { .. }
    ));
    assert!(matches!(
        parse_catalog_coordinate(&format!("5129:{event_id}:{author}")).unwrap(),
        ManifestCoordinate::Snapshot { .. }
    ));
    for invalid in [
        "",
        "35129:author",
        "15129:author:extra",
        "unknown:author:d-tag",
        " 35129:author:d-tag",
    ] {
        assert!(
            parse_catalog_coordinate(invalid).is_err(),
            "unexpectedly accepted {invalid:?}"
        );
    }
}

/// End-to-end proof that NAP-INTENT dispatch is real, not hardcoded: an
/// installed-but-never-launched handler napplet gets launched by the
/// dispatcher itself in reaction to a caller's `intent.invoke`, and once
/// the (test-simulated) handler subscribes to its declared convention
/// topic, receives the invocation payload as a real `inc.event` push and
/// the caller receives a matching `ok:true` `intent.invoke.result`.
#[test]
fn intent_invoke_launches_a_registered_handler_and_delivers_the_payload_via_inc() {
    let temp = TempDir::new().unwrap();
    let (handler_event, handler_author, handler_digest) = signed_manifest_event(
        "nip29-chat-test",
        b"<html>handler</html>",
        vec![
            vec!["requires".to_owned(), "intent".to_owned()],
            vec!["requires".to_owned(), "inc".to_owned()],
            vec![
                "archetype".to_owned(),
                "nip29-group".to_owned(),
                "napplet:nip29-group/open".to_owned(),
            ],
        ],
    );
    let (caller_event, caller_author, caller_digest) = signed_manifest_event(
        "nip29-groups-test",
        b"<html>caller</html>",
        vec![vec!["requires".to_owned(), "intent".to_owned()]],
    );

    let controller = RuntimeController::open(
        RuntimeConfig {
            runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
            nmp_store_path: None,
            artifact_cache_path: temp.path().join("artifacts").display().to_string(),
            ..RuntimeConfig::default()
        },
        Box::new(FixtureSource(BTreeMap::from([
            (handler_digest, b"<html>handler</html>".to_vec()),
            (caller_digest, b"<html>caller</html>".to_vec()),
        ]))),
    )
    .unwrap();

    // Install the handler but never launch it -- the dispatcher itself
    // must be the one that launches it.
    let handler_artifact = controller
        .verify_artifact(
            handler_event,
            ArtifactCoordinate::Named {
                author: handler_author,
                d_tag: "nip29-chat-test".to_owned(),
            },
        )
        .artifact
        .expect("handler manifest verifies");
    controller.install(Arc::clone(&handler_artifact));
    for domain in ["intent", "inc"] {
        controller.set_grant(
            Arc::clone(&handler_artifact),
            domain.to_owned(),
            RuntimeSensitivity::Sensitive,
            RuntimeGrantDecision::AllowExactBuild,
        );
    }

    // Install, grant, and launch the caller.
    let caller_artifact = controller
        .verify_artifact(
            caller_event,
            ArtifactCoordinate::Named {
                author: caller_author,
                d_tag: "nip29-groups-test".to_owned(),
            },
        )
        .artifact
        .expect("caller manifest verifies");
    controller.install(Arc::clone(&caller_artifact));
    controller.set_grant(
        Arc::clone(&caller_artifact),
        "intent".to_owned(),
        RuntimeSensitivity::Sensitive,
        RuntimeGrantDecision::AllowExactBuild,
    );
    controller.launch(
        Arc::clone(&caller_artifact),
        RuntimeExecutionProfile::Legacy,
    );
    let caller_session = controller.snapshot().sessions[0].id;
    controller.mapped_envelope(caller_session, br#"{"type":"shell.ready"}"#.to_vec());

    controller.mapped_envelope(
        caller_session,
        serde_json::to_vec(&serde_json::json!({
            "type": "intent.invoke",
            "id": "invoke-1",
            "request": {
                "archetype": "nip29-group",
                "convention": "napplet:nip29-group/open",
                "payload": {"group": "abc"}
            }
        }))
        .unwrap(),
    );

    // The dispatcher launches the handler on a background thread; poll
    // for its session to appear.
    let deadline = Instant::now() + Duration::from_secs(5);
    let handler_session = loop {
        if let Some(session) = controller
            .snapshot()
            .sessions
            .iter()
            .find(|session| session.id != caller_session)
        {
            break session.id;
        }
        assert!(Instant::now() < deadline, "handler session never launched");
        thread::sleep(Duration::from_millis(20));
    };

    // Simulate the handler napplet's own JS boot: ready, then subscribe
    // to the exact convention it declared in its manifest.
    controller.mapped_envelope(handler_session, br#"{"type":"shell.ready"}"#.to_vec());
    controller.mapped_envelope(
        handler_session,
        serde_json::to_vec(&serde_json::json!({
            "type": "inc.subscribe",
            "id": "sub-1",
            "topic": "napplet:nip29-group/open"
        }))
        .unwrap(),
    );

    // The dispatcher's poll loop should now deliver the payload as a
    // real `inc.event` push and resolve the caller's invocation.
    let deadline = Instant::now() + Duration::from_secs(5);
    let event = loop {
        if let Some(event) = controller
            .app
            .events_after(0)
            .events
            .into_iter()
            .find_map(|event| match event.event {
                PlatformEvent::ProviderPush {
                    session, envelope, ..
                } if session == SessionId(handler_session)
                    && envelope.decode().ok()?.get("type")? == "inc.event" =>
                {
                    envelope.decode().ok()
                }
                _ => None,
            })
        {
            break event;
        }
        assert!(
            Instant::now() < deadline,
            "handler never received the inc.event push"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(event["topic"], "napplet:nip29-group/open");
    assert_eq!(event["sender"], "nip29-groups-test");
    assert_eq!(event["payload"], serde_json::json!({"group": "abc"}));

    // `intent.invoke.result` is delivered asynchronously as a provider push
    // to the caller's session (mirroring `inc.event` above), not as a
    // synchronous `EnvelopeHandled` response to the original `intent.invoke`
    // call -- `IntentProvider::invoke` returns immediately with no response
    // and only pushes the result once `complete()` runs.
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        if let Some(result) = controller
            .app
            .events_after(0)
            .events
            .into_iter()
            .find_map(|event| match event.event {
                PlatformEvent::ProviderPush {
                    session, envelope, ..
                } if session == SessionId(caller_session)
                    && envelope.decode().ok()?.get("type")? == "intent.invoke.result" =>
                {
                    envelope.decode().ok()
                }
                _ => None,
            })
        {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "caller never received intent.invoke.result"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(result["id"], "invoke-1");
    assert_eq!(result["result"]["ok"], true);
    assert_eq!(result["result"]["handled"], true);
    assert_eq!(result["result"]["archetype"], "nip29-group");
}
