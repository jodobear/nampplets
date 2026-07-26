//! Cross-publication invariants for producer-side section revisions.

mod support;

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use nmp_native_runtime_app::{AppObserver, AppSnapshot, PlatformCommand};
use nmp_native_runtime_store::WorkspaceRecord;
use support::*;

fn next_observation(observer: &mut AppObserver) -> Arc<AppSnapshot> {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(2), observer.changed())
                .await
                .expect("event-driven app observation timed out")
                .expect("runtime observation closed unexpectedly")
        })
}

fn assert_sound_negative(before: &AppSnapshot, after: &AppSnapshot) {
    macro_rules! unchanged_proves {
        ($revision:ident, $content:expr) => {
            if before.revisions.$revision == after.revisions.$revision {
                assert!(
                    $content,
                    concat!(stringify!($revision), " content changed silently")
                );
            }
        };
    }
    unchanged_proves!(library, before.library == after.library);
    unchanged_proves!(
        sessions,
        before.sessions == after.sessions && before.session_domains == after.session_domains
    );
    unchanged_proves!(
        provider_push_lanes,
        before.provider_push_lanes == after.provider_push_lanes
    );
    unchanged_proves!(bindings, before.bindings == after.bindings);
    unchanged_proves!(
        pending_writes,
        before.pending_writes == after.pending_writes
    );
    unchanged_proves!(receipts, before.receipts == after.receipts);
    unchanged_proves!(workspaces, before.workspaces == after.workspaces);
    unchanged_proves!(resources, before.resources == after.resources);
    unchanged_proves!(
        activity,
        before.recent_activity == after.recent_activity
            && before.dropped_activity == after.dropped_activity
    );
    unchanged_proves!(
        errors,
        before.recent_errors == after.recent_errors
            && before.dropped_errors == after.dropped_errors
    );
}

#[test]
fn observer_snapshots_keep_the_sound_negative_contract_across_a_runtime_flow() {
    let rig = Rig::new(false);
    let exact = principal('b');
    let mut observer = rig.app.observe();
    let mut previous = observer.latest();

    rig.install(exact.clone());
    let mut current = next_observation(&mut observer);
    assert_sound_negative(&previous, &current);
    previous = current;

    rig.allow_runtime(exact.clone());
    current = next_observation(&mut observer);
    assert_sound_negative(&previous, &current);
    previous = current;

    let session = rig.launch(exact);
    current = next_observation(&mut observer);
    assert_sound_negative(&previous, &current);
    previous = current;

    rig.ready(session);
    current = next_observation(&mut observer);
    assert_sound_negative(&previous, &current);
    previous = current;

    rig.provider
        .sender(session)
        .push("canary.state", serde_json::Map::new(), None)
        .unwrap();
    current = next_observation(&mut observer);
    assert_sound_negative(&previous, &current);
    previous = current;

    rig.app.dispatch(PlatformCommand::SaveWorkspace {
        workspace: WorkspaceRecord {
            id: Arc::from("main"),
            definition: json(serde_json::json!({"layout": "single"})),
            retained_receipts: Vec::new(),
        },
    });
    current = next_observation(&mut observer);
    assert_sound_negative(&previous, &current);
    previous = current;

    rig.app.dispatch(PlatformCommand::Close);
    current = next_observation(&mut observer);
    assert_sound_negative(&previous, &current);
    assert!(current.closed, "closed is observed outside section gating");
}

#[test]
fn ring_revisions_equal_their_monotonic_appended_counts() {
    let rig = Rig::new(false);
    for hash in ['b', 'c', 'd'] {
        rig.install(principal(hash));
    }
    for hash in ['e', 'f'] {
        rig.app.dispatch(PlatformCommand::Launch {
            principal: principal(hash),
            profile: nmp_native_runtime_core::ExecutionProfile::Legacy,
            required_domains: BTreeSet::from([canary()]),
        });
    }
    let snapshot = rig.app.snapshot();
    assert_eq!(
        snapshot.revisions.activity,
        snapshot.recent_activity.len() as u64 + snapshot.dropped_activity
    );
    assert_eq!(
        snapshot.revisions.errors,
        snapshot.recent_errors.len() as u64 + snapshot.dropped_errors
    );
}
