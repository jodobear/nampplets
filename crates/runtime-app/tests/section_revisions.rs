//! Producer-side section revision contract and independence tests.

mod support;

use std::{collections::BTreeSet, sync::Arc};

use nmp_native_runtime_app::{AppSnapshot, PlatformCommand, PlatformEvent};
use nmp_native_runtime_store::WorkspaceRecord;
use support::*;

fn changed_sections(before: &AppSnapshot, after: &AppSnapshot) -> BTreeSet<&'static str> {
    let mut changed = BTreeSet::new();
    let pairs = [
        ("library", before.revisions.library, after.revisions.library),
        (
            "sessions",
            before.revisions.sessions,
            after.revisions.sessions,
        ),
        (
            "provider_push_lanes",
            before.revisions.provider_push_lanes,
            after.revisions.provider_push_lanes,
        ),
        (
            "bindings",
            before.revisions.bindings,
            after.revisions.bindings,
        ),
        (
            "pending_writes",
            before.revisions.pending_writes,
            after.revisions.pending_writes,
        ),
        (
            "receipts",
            before.revisions.receipts,
            after.revisions.receipts,
        ),
        (
            "workspaces",
            before.revisions.workspaces,
            after.revisions.workspaces,
        ),
        (
            "resources",
            before.revisions.resources,
            after.revisions.resources,
        ),
        (
            "activity",
            before.revisions.activity,
            after.revisions.activity,
        ),
        ("errors", before.revisions.errors, after.revisions.errors),
        (
            "newest_event_sequence",
            before.revisions.newest_event_sequence,
            after.revisions.newest_event_sequence,
        ),
    ];
    for (name, before, after) in pairs {
        assert!(after >= before, "{name} revision must be monotonic");
        if after != before {
            changed.insert(name);
        }
    }
    changed
}

fn assert_structural_revisions_match_content(before: &AppSnapshot, after: &AppSnapshot) {
    let exact = [
        (
            "library",
            before.revisions.library != after.revisions.library,
            before.library != after.library,
        ),
        (
            "sessions",
            before.revisions.sessions != after.revisions.sessions,
            before.sessions != after.sessions || before.session_domains != after.session_domains,
        ),
        (
            "provider_push_lanes",
            before.revisions.provider_push_lanes != after.revisions.provider_push_lanes,
            before.provider_push_lanes != after.provider_push_lanes,
        ),
        (
            "bindings",
            before.revisions.bindings != after.revisions.bindings,
            before.bindings != after.bindings,
        ),
        (
            "pending_writes",
            before.revisions.pending_writes != after.revisions.pending_writes,
            before.pending_writes != after.pending_writes,
        ),
        (
            "receipts",
            before.revisions.receipts != after.revisions.receipts,
            before.receipts != after.receipts,
        ),
        (
            "workspaces",
            before.revisions.workspaces != after.revisions.workspaces,
            before.workspaces != after.workspaces,
        ),
        (
            "resources",
            before.revisions.resources != after.revisions.resources,
            before.resources != after.resources,
        ),
        (
            "activity",
            before.revisions.activity != after.revisions.activity,
            before.recent_activity != after.recent_activity
                || before.dropped_activity != after.dropped_activity,
        ),
        (
            "errors",
            before.revisions.errors != after.revisions.errors,
            before.recent_errors != after.recent_errors
                || before.dropped_errors != after.dropped_errors,
        ),
    ];
    for (name, revision_advanced, content_changed) in exact {
        assert_eq!(
            revision_advanced, content_changed,
            "{name} revision must advance if and only if its content changes under v1 equality"
        );
    }
}

#[test]
fn provider_push_advances_only_its_lane_and_event_sections() {
    let rig = Rig::new(false);
    let exact = principal('b');
    rig.install(exact.clone());
    rig.allow_runtime(exact.clone());
    let session = rig.launch(exact);
    rig.ready(session);
    let sender = rig.provider.sender(session);
    let before = rig.app.snapshot();

    sender
        .push(
            "canary.state",
            serde_json::Map::from_iter([("value".to_owned(), serde_json::json!(1))]),
            None,
        )
        .unwrap();
    let _ = wait_for_event(&rig.app, |event| {
        matches!(
            event,
            PlatformEvent::ProviderPush {
                session: pushed_session,
                ..
            } if *pushed_session == session
        )
    });
    let after = rig.app.snapshot();

    assert_structural_revisions_match_content(&before, &after);
    assert_eq!(
        changed_sections(&before, &after),
        BTreeSet::from(["newest_event_sequence", "provider_push_lanes"])
    );
}

#[test]
fn workspace_publication_does_not_dirty_unrelated_sections() {
    let rig = Rig::new(false);
    let workspace = WorkspaceRecord {
        id: Arc::from("main"),
        definition: json(serde_json::json!({"layout": "two-up"})),
        retained_receipts: Vec::new(),
    };
    let before = rig.app.snapshot();

    rig.app.dispatch(PlatformCommand::SaveWorkspace {
        workspace: workspace.clone(),
    });
    let saved = rig.app.snapshot();
    assert_structural_revisions_match_content(&before, &saved);
    assert_eq!(
        changed_sections(&before, &saved),
        BTreeSet::from(["newest_event_sequence", "workspaces"])
    );

    // This exact no-op assertion is specific to v1 section equality. A future
    // mutator-attributed producer may conservatively advance `workspaces`.
    rig.app
        .dispatch(PlatformCommand::SaveWorkspace { workspace });
    let resaved = rig.app.snapshot();
    assert_structural_revisions_match_content(&saved, &resaved);
    assert_eq!(
        changed_sections(&saved, &resaved),
        BTreeSet::from(["newest_event_sequence"])
    );
}

#[test]
fn closed_runtime_refusal_advances_only_error_and_event_sections() {
    let rig = Rig::new(false);
    rig.app.dispatch(PlatformCommand::Close);
    let before = rig.app.snapshot();

    rig.app.dispatch(PlatformCommand::SetLibraryFilter {
        query: Arc::from("ignored"),
    });
    let after = rig.app.snapshot();

    assert_structural_revisions_match_content(&before, &after);
    assert_eq!(
        changed_sections(&before, &after),
        BTreeSet::from(["errors", "newest_event_sequence"])
    );
}
