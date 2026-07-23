use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use nmp_native_runtime_core::{
    BindingEventSink, BindingSinkError, HostBindingHandle, HostBindingSnapshot,
};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingLimits {
    pub maximum_consumers: usize,
    pub maximum_frame_bytes: usize,
}

impl Default for BindingLimits {
    fn default() -> Self {
        Self {
            maximum_consumers: 16,
            maximum_frame_bytes: 512 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingSnapshot {
    pub revision: u64,
    pub value: nmp_native_runtime_core::BoundedJson,
    pub scoped_evidence: nmp_native_runtime_core::BoundedJson,
}

pub struct Binding {
    id: Arc<str>,
    schema: Arc<str>,
    limits: BindingLimits,
    latest: Mutex<Option<Arc<BindingSnapshot>>>,
    sender: watch::Sender<Option<Arc<BindingSnapshot>>>,
    source: Mutex<Option<Arc<dyn HostBindingHandle>>>,
    consumers: Arc<AtomicUsize>,
    closed: AtomicBool,
}

impl fmt::Debug for Binding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Binding")
            .field("id", &self.id)
            .field("schema", &self.schema)
            .field(
                "latest_revision",
                &self.latest().map(|value| value.revision),
            )
            .field("consumers", &self.consumer_count())
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish()
    }
}

impl Binding {
    pub fn new(
        id: impl Into<Arc<str>>,
        schema: impl Into<Arc<str>>,
        limits: BindingLimits,
    ) -> Result<Arc<Self>, BindingError> {
        if limits.maximum_consumers == 0 || limits.maximum_frame_bytes == 0 {
            return Err(BindingError::InvalidLimits);
        }
        let (sender, _) = watch::channel(None);
        Ok(Arc::new(Self {
            id: id.into(),
            schema: schema.into(),
            limits,
            latest: Mutex::new(None),
            sender,
            source: Mutex::new(None),
            consumers: Arc::new(AtomicUsize::new(0)),
            closed: AtomicBool::new(false),
        }))
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn attach_source(&self, source: Arc<dyn HostBindingHandle>) -> Result<(), BindingError> {
        if self.closed.load(Ordering::Acquire) {
            source.close();
            return Err(BindingError::Closed);
        }
        let mut owner = self.source.lock();
        if owner.is_some() {
            source.close();
            return Err(BindingError::SourceAlreadyAttached);
        }
        *owner = Some(source);
        Ok(())
    }

    pub fn logical_source_id(&self) -> Option<String> {
        self.source
            .lock()
            .as_ref()
            .map(|source| source.logical_id().to_owned())
    }

    pub fn latest(&self) -> Option<Arc<BindingSnapshot>> {
        self.latest.lock().clone()
    }

    pub fn subscribe(&self) -> Result<BindingConsumer, BindingError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(BindingError::Closed);
        }
        let mut current = self.consumers.load(Ordering::Acquire);
        loop {
            if current >= self.limits.maximum_consumers {
                return Err(BindingError::ConsumerCapacity {
                    capacity: self.limits.maximum_consumers,
                });
            }
            match self.consumers.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(updated) => current = updated,
            }
        }
        Ok(BindingConsumer {
            receiver: self.sender.subscribe(),
            consumers: Arc::clone(&self.consumers),
        })
    }

    pub fn consumer_count(&self) -> usize {
        self.consumers.load(Ordering::Acquire)
    }

    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(source) = self.source.lock().take() {
            source.close();
        }
    }
}

impl BindingEventSink for Binding {
    fn push_latest(&self, snapshot: HostBindingSnapshot) -> Result<(), BindingSinkError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(BindingSinkError::Closed);
        }
        if snapshot.value.byte_len() + snapshot.scoped_evidence.byte_len()
            > self.limits.maximum_frame_bytes
        {
            return Err(BindingSinkError::FrameTooLarge);
        }
        let next = Arc::new(BindingSnapshot {
            revision: snapshot.source_generation,
            value: snapshot.value,
            scoped_evidence: snapshot.scoped_evidence,
        });
        let mut latest = self.latest.lock();
        if latest
            .as_ref()
            .is_some_and(|current| current.revision >= next.revision)
        {
            return Ok(());
        }
        *latest = Some(Arc::clone(&next));
        self.sender.send_replace(Some(next));
        Ok(())
    }

    fn close(&self, _reason: Option<Arc<str>>) {
        Binding::close(self);
    }
}

impl Drop for Binding {
    fn drop(&mut self) {
        if let Some(source) = self.source.get_mut().take() {
            source.close();
        }
    }
}

#[derive(Debug)]
pub struct BindingConsumer {
    receiver: watch::Receiver<Option<Arc<BindingSnapshot>>>,
    consumers: Arc<AtomicUsize>,
}

impl BindingConsumer {
    pub fn latest(&self) -> Option<Arc<BindingSnapshot>> {
        self.receiver.borrow().clone()
    }

    pub async fn changed(&mut self) -> Result<Arc<BindingSnapshot>, BindingError> {
        loop {
            self.receiver
                .changed()
                .await
                .map_err(|_| BindingError::Closed)?;
            if let Some(snapshot) = self.receiver.borrow_and_update().clone() {
                return Ok(snapshot);
            }
        }
    }
}

impl Drop for BindingConsumer {
    fn drop(&mut self) {
        self.consumers.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RendererId(pub Arc<str>);

#[derive(Debug)]
pub struct RendererSlot {
    binding: Arc<Binding>,
    state: Mutex<RendererSlotState>,
}

#[derive(Debug, Default)]
struct RendererSlotState {
    current: Option<RendererId>,
    pending: Option<RendererId>,
}

impl RendererSlot {
    pub fn new(binding: Arc<Binding>, current: Option<RendererId>) -> Self {
        Self {
            binding,
            state: Mutex::new(RendererSlotState {
                current,
                pending: None,
            }),
        }
    }

    pub fn binding(&self) -> &Arc<Binding> {
        &self.binding
    }

    pub fn begin_replace(&self, renderer: RendererId) -> Result<(), BindingError> {
        let mut state = self.state.lock();
        if state.pending.is_some() {
            return Err(BindingError::ReplacementAlreadyPending);
        }
        state.pending = Some(renderer);
        Ok(())
    }

    /// The old renderer is returned only after the replacement reports ready.
    pub fn replacement_ready(&self, renderer: &RendererId) -> Result<RendererSwap, BindingError> {
        let mut state = self.state.lock();
        if state.pending.as_ref() != Some(renderer) {
            return Err(BindingError::UnexpectedRendererReady);
        }
        let unmounted = state.current.replace(renderer.clone());
        state.pending = None;
        Ok(RendererSwap {
            mounted: renderer.clone(),
            unmounted,
            binding_id: Arc::clone(&self.binding.id),
            binding_revision: self.binding.latest().map(|snapshot| snapshot.revision),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RendererSwap {
    pub mounted: RendererId,
    pub unmounted: Option<RendererId>,
    pub binding_id: Arc<str>,
    pub binding_revision: Option<u64>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BindingError {
    #[error("binding limits must be finite and non-zero")]
    InvalidLimits,
    #[error("binding is closed")]
    Closed,
    #[error("binding source is already attached")]
    SourceAlreadyAttached,
    #[error("binding consumer capacity {capacity} is full")]
    ConsumerCapacity { capacity: usize },
    #[error("renderer replacement is already pending")]
    ReplacementAlreadyPending,
    #[error("ready message does not match the pending renderer")]
    UnexpectedRendererReady,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use nmp_native_runtime_core::{BoundedJson, HostBindingSnapshot};

    use super::*;

    #[derive(Debug)]
    struct TestHandle {
        id: String,
        closed: AtomicBool,
    }

    impl HostBindingHandle for TestHandle {
        fn logical_id(&self) -> &str {
            &self.id
        }

        fn close(&self) {
            self.closed.store(true, Ordering::Release);
        }
    }

    fn update(revision: u64, count: usize) -> HostBindingSnapshot {
        HostBindingSnapshot {
            source_generation: revision,
            value: BoundedJson::from_value(&serde_json::json!({"items": vec![1; count]}), 100_000)
                .unwrap(),
            scoped_evidence: BoundedJson::from_value(&serde_json::json!({"sources": []}), 100_000)
                .unwrap(),
        }
    }

    #[test]
    fn native_and_web_consumers_share_one_revision() {
        let binding = Binding::new(
            "feed",
            "nostr.events.collection/1",
            BindingLimits::default(),
        )
        .unwrap();
        let native = binding.subscribe().unwrap();
        let web = binding.subscribe().unwrap();
        binding.push_latest(update(7, 2)).unwrap();

        assert_eq!(native.latest().unwrap().revision, 7);
        assert_eq!(web.latest().unwrap().revision, 7);
        assert_eq!(binding.consumer_count(), 2);
    }

    #[test]
    fn slow_consumer_conflates_to_latest_snapshot() {
        let binding = Binding::new(
            "feed",
            "nostr.events.collection/1",
            BindingLimits::default(),
        )
        .unwrap();
        let slow = binding.subscribe().unwrap();
        for revision in 1..=1_000 {
            binding.push_latest(update(revision, 1)).unwrap();
        }
        assert_eq!(slow.latest().unwrap().revision, 1_000);
    }

    #[test]
    fn renderer_swap_preserves_binding_and_source_identity() {
        let binding = Binding::new(
            "feed",
            "nostr.events.collection/1",
            BindingLimits::default(),
        )
        .unwrap();
        binding
            .attach_source(Arc::new(TestHandle {
                id: "nmp-observation-1".to_owned(),
                closed: AtomicBool::new(false),
            }))
            .unwrap();
        binding.push_latest(update(7, 2)).unwrap();
        let slot = RendererSlot::new(
            Arc::clone(&binding),
            Some(RendererId(Arc::from("renderer-a"))),
        );
        let replacement = RendererId(Arc::from("renderer-b"));
        slot.begin_replace(replacement.clone()).unwrap();
        assert_eq!(
            slot.binding().logical_source_id().as_deref(),
            Some("nmp-observation-1")
        );
        let swap = slot.replacement_ready(&replacement).unwrap();
        assert_eq!(swap.binding_revision, Some(7));
        assert_eq!(swap.unmounted, Some(RendererId(Arc::from("renderer-a"))));
        assert_eq!(
            slot.binding().logical_source_id().as_deref(),
            Some("nmp-observation-1")
        );
    }
}
