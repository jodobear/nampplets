use std::{sync::mpsc, time::Duration};

use nmp_native_runtime_app::{AppTerminalReason, PlatformCommand, SnapshotSection};
use nmp_native_runtime_core::BoundedJson;
use nmp_native_runtime_store::WorkspaceRecord;

use super::*;
use crate::slots::project_receipts;

struct ReceiptSlotRecorder(mpsc::Sender<RuntimeReceiptsSlotProjection>);

impl RuntimeReceiptsSlotObserver for ReceiptSlotRecorder {
    fn update(&self, projection: RuntimeReceiptsSlotProjection) {
        let _ = self.0.send(projection);
    }
}

struct FusedNoop;

impl RuntimeObserver for FusedNoop {
    fn update(&self, _frame: RuntimeObservationFrame) {}
}

#[test]
fn receipt_slot_ignores_unrelated_sections_but_always_delivers_close() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);
    let (send, receive) = mpsc::channel();
    let start = controller
        .clone()
        .observe_receipts(Box::new(ReceiptSlotRecorder(send)));
    assert!(start.refusal.is_none());
    let observation = start.observation.expect("receipt observer admitted");
    let initial = match start.initial.expect("authoritative initial projection") {
        RuntimeReceiptsSlotProjection::Snapshot { snapshot } => snapshot,
        RuntimeReceiptsSlotProjection::Refused { refusal, .. } => {
            panic!("healthy receipt slot refused: {refusal:?}")
        }
    };
    assert!(!initial.closed);
    assert!(initial.receipts.is_empty());

    controller.app.dispatch(PlatformCommand::SaveWorkspace {
        workspace: WorkspaceRecord {
            id: "unrelated".into(),
            definition: BoundedJson::from_value(&serde_json::json!({}), 4_096).unwrap(),
            retained_receipts: Vec::new(),
        },
    });
    assert!(
        receive.recv_timeout(Duration::from_millis(100)).is_err(),
        "workspace-only publication reached the receipts slot"
    );

    controller.close();
    let closed = receive
        .recv_timeout(Duration::from_secs(2))
        .expect("receipt slot did not deliver its final closed state");
    assert!(matches!(
        closed,
        RuntimeReceiptsSlotProjection::Snapshot {
            snapshot: RuntimeReceiptsSlotSnapshot { closed: true, .. }
        }
    ));
    observation.stop();
}

#[test]
fn legacy_and_slot_observers_share_one_global_capacity() {
    let temp = TempDir::new().unwrap();
    let controller = RuntimeController::open(
        RuntimeConfig {
            runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
            artifact_cache_path: temp.path().join("artifacts").display().to_string(),
            maximum_observers: 1,
            ..RuntimeConfig::default()
        },
        Box::new(FixtureSource(BTreeMap::from([(
            DIGEST.to_owned(),
            INDEX.to_vec(),
        )]))),
    )
    .unwrap();

    let fused = controller
        .clone()
        .observe(Box::new(FusedNoop))
        .observation
        .expect("legacy observer admitted");
    let (send, _receive) = mpsc::channel();
    let refused = controller
        .clone()
        .observe_receipts(Box::new(ReceiptSlotRecorder(send)));
    assert!(refused.observation.is_none());
    assert!(refused.initial.is_none());
    assert_eq!(
        refused.refusal.expect("capacity refusal").code,
        "receipts-slot-observer-capacity"
    );
    fused.stop();
}

#[test]
fn receipt_revision_exhaustion_is_a_typed_terminal_refusal() {
    let temp = TempDir::new().unwrap();
    let controller = controller(&temp);
    let mut source = (*controller.app.snapshot()).clone();
    source.closed = true;
    source.revisions.receipts = u64::MAX;
    source.terminal_reason = Some(AppTerminalReason::SectionRevisionExhausted {
        section: SnapshotSection::Receipts,
    });

    assert!(matches!(
        project_receipts(&controller, &source),
        RuntimeReceiptsSlotProjection::Refused {
            revision: u64::MAX,
            closed: true,
            refusal: RuntimeRefusal { code, .. },
        } if code == "receipts-slot-revision-exhausted"
    ));
}
