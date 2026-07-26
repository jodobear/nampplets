//! Producer-side section revision contract and independence tests.

mod support;

use nmp_native_runtime_app::PlatformEvent;
use support::*;

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

    assert_eq!(after.revisions.library, before.revisions.library);
    assert_eq!(after.revisions.sessions, before.revisions.sessions);
    assert_eq!(after.revisions.bindings, before.revisions.bindings);
    assert_eq!(
        after.revisions.pending_writes,
        before.revisions.pending_writes
    );
    assert_eq!(after.revisions.receipts, before.revisions.receipts);
    assert_eq!(after.revisions.workspaces, before.revisions.workspaces);
    assert_eq!(after.revisions.resources, before.revisions.resources);
    assert_eq!(after.revisions.activity, before.revisions.activity);
    assert_eq!(after.revisions.errors, before.revisions.errors);
    assert!(
        after.revisions.provider_push_lanes > before.revisions.provider_push_lanes,
        "the live push lane itself must advance"
    );
    assert!(
        after.revisions.newest_event_sequence > before.revisions.newest_event_sequence,
        "the incremental event cursor must advance"
    );
}
