//! Count-based provider-push attribution measurement for issue #129.

mod support;

use std::{sync::Arc, time::Duration};

use nmp_native_runtime_app::{AppObserver, AppSnapshot};
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

#[test]
fn sequential_provider_pushes_measure_scoped_revision_attribution() {
    const PUSH_BATCHES: u64 = 64;
    const SECTION_COUNT: u64 = 11;

    let rig = Rig::new(false);
    let exact = principal('b');
    rig.install(exact.clone());
    rig.allow_runtime(exact.clone());
    let session = rig.launch(exact);
    rig.ready(session);
    let sender = rig.provider.sender(session);
    let mut observer = rig.app.observe();
    let before = observer.latest();
    let mut app_wakeups = 0_u64;

    for sample in 0..PUSH_BATCHES {
        let sequence = sender
            .push(
                "canary.state",
                serde_json::Map::from_iter([("sample".to_owned(), sample.into())]),
                None,
            )
            .unwrap();
        let snapshot = next_observation(&mut observer);
        app_wakeups += 1;
        assert_eq!(
            snapshot.provider_push_lanes[0].last_provider_sequence,
            Some(sequence)
        );
    }
    let after = observer.latest();

    let fused_publications = after.revision - before.revision;
    let deltas = [
        after.revisions.library - before.revisions.library,
        after.revisions.sessions - before.revisions.sessions,
        after.revisions.provider_push_lanes - before.revisions.provider_push_lanes,
        after.revisions.bindings - before.revisions.bindings,
        after.revisions.pending_writes - before.revisions.pending_writes,
        after.revisions.receipts - before.revisions.receipts,
        after.revisions.workspaces - before.revisions.workspaces,
        after.revisions.resources - before.revisions.resources,
        after.revisions.activity - before.revisions.activity,
        after.revisions.errors - before.revisions.errors,
        after.revisions.newest_event_sequence - before.revisions.newest_event_sequence,
    ];
    let candidate_section_advances = deltas.into_iter().sum::<u64>();
    let baseline_fused_invalidations = fused_publications * SECTION_COUNT;

    assert_eq!(fused_publications, PUSH_BATCHES);
    assert_eq!(app_wakeups, PUSH_BATCHES);
    assert_eq!(
        after.revisions.provider_push_lanes - before.revisions.provider_push_lanes,
        PUSH_BATCHES
    );
    assert_eq!(
        after.revisions.newest_event_sequence - before.revisions.newest_event_sequence,
        PUSH_BATCHES
    );
    assert_eq!(candidate_section_advances, PUSH_BATCHES * 2);

    eprintln!(
        "samples={PUSH_BATCHES} fused_publications={fused_publications} \
         app_wakeups={app_wakeups} baseline_fused_section_invalidations=\
         {baseline_fused_invalidations} candidate_section_advances=\
         {candidate_section_advances}"
    );
}
