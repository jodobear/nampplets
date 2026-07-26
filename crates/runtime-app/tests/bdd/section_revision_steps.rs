//! Steps for producer-side section revision independence.

use std::{collections::BTreeSet, time::Duration};

use cucumber::{given, then, when};
use nmp_native_runtime_app::PlatformEvent;

use super::RuntimeWorld;

#[given("the current producer section revisions have been recorded")]
fn given_section_revisions_recorded(world: &mut RuntimeWorld) {
    world.before_revisions = Some(world.rig().app.snapshot().revisions);
}

#[when("the provider delivers a valid live canary state update")]
async fn when_valid_live_update(world: &mut RuntimeWorld) {
    let session = world.session.unwrap();
    let app = world.rig().app.clone();
    let mut observer = app.observe();
    world
        .sender
        .clone()
        .unwrap()
        .push("canary.state", serde_json::Map::new(), None)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if app.events_after(0).events.into_iter().any(|event| {
                matches!(
                    event.event,
                    PlatformEvent::ProviderPush {
                        session: pushed_session,
                        ..
                    } if pushed_session == session
                )
            }) {
                break;
            }
            observer.changed().await.unwrap();
        }
    })
    .await
    .expect("event-driven provider update timed out");
    world.after_revisions = Some(app.snapshot().revisions);
}

#[then("only live delivery and event replay are marked changed")]
fn then_only_delivery_and_event_changed(world: &mut RuntimeWorld) {
    let before = world.before_revisions.unwrap();
    let after = world.after_revisions.unwrap();
    let unchanged = [
        ("library", before.library, after.library),
        ("sessions", before.sessions, after.sessions),
        ("bindings", before.bindings, after.bindings),
        (
            "pending writes",
            before.pending_writes,
            after.pending_writes,
        ),
        ("receipts", before.receipts, after.receipts),
        ("workspaces", before.workspaces, after.workspaces),
        ("resources", before.resources, after.resources),
        ("activity", before.activity, after.activity),
        ("errors", before.errors, after.errors),
    ];
    let dirty = unchanged
        .into_iter()
        .filter_map(|(name, old, new)| (old != new).then_some(name))
        .collect::<BTreeSet<_>>();
    assert!(dirty.is_empty(), "unrelated dirty sections: {dirty:?}");
    assert!(after.provider_push_lanes > before.provider_push_lanes);
    assert!(after.newest_event_sequence > before.newest_event_sequence);
}
