use std::{
    collections::{BTreeSet, VecDeque},
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use nmp_native_runtime_core::{BoundedJson, Capability, Principal, SessionId};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use tokio::sync::watch;

const RESERVED_AUTHORITY_FIELDS: &[&str] = &[
    "principal",
    "session",
    "sessionId",
    "sourceWindow",
    "sourceWindowId",
    "manifestAuthor",
    "dTag",
    "aggregateHash",
];

/// Opaque routing identity assigned by the trusted runtime when it creates a
/// component window. It is never parsed from a component envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceWindowId(pub u64);

/// Capacity contract for one mapped session's outbound provider lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderPushLimits {
    pub maximum_pending_count: usize,
    pub maximum_pending_bytes: usize,
    pub maximum_envelope_bytes: usize,
    pub maximum_conflation_key_bytes: usize,
}

impl Default for ProviderPushLimits {
    fn default() -> Self {
        Self {
            maximum_pending_count: 64,
            maximum_pending_bytes: 512 * 1024,
            maximum_envelope_bytes: 256 * 1024,
            maximum_conflation_key_bytes: 256,
        }
    }
}

impl ProviderPushLimits {
    pub(crate) fn validate(self) -> bool {
        self.maximum_pending_count > 0
            && self.maximum_pending_bytes > 0
            && self.maximum_envelope_bytes > 0
            && self.maximum_conflation_key_bytes > 0
            && self.maximum_envelope_bytes <= self.maximum_pending_bytes
    }
}

/// One provider-authored wire envelope plus trusted out-of-band routing.
///
/// `envelope` contains only the NAP message. Runtime authority is deliberately
/// absent and cannot be spoofed by provider or component fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderPush {
    pub sequence: u64,
    pub session: SessionId,
    pub source_window: SourceWindowId,
    pub domain: Capability,
    pub envelope: BoundedJson,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderPushBatch {
    pub pushes: Vec<ProviderPush>,
    pub closed: bool,
    pub termination: Option<ProviderPushTermination>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderPushTermination {
    Backpressure,
    ProviderFailure,
}

/// A capability-scoped producer handed to a provider only for an exact mapped
/// session. Clones retain no queue ownership and become closed on revoke or
/// session teardown.
#[derive(Clone)]
pub struct ProviderPushSender {
    session: SessionId,
    source_window: SourceWindowId,
    domain: Capability,
    mailbox: Weak<OutboundMailbox>,
}

impl fmt::Debug for ProviderPushSender {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderPushSender")
            .field("session", &self.session)
            .field("source_window", &self.source_window)
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}

impl ProviderPushSender {
    pub fn session(&self) -> SessionId {
        self.session
    }

    pub fn source_window(&self) -> SourceWindowId {
        self.source_window
    }

    pub fn domain(&self) -> &Capability {
        &self.domain
    }

    /// Enqueues an unsolicited NAP envelope without blocking.
    ///
    /// A conflation key means "latest state for this key". Replacing an
    /// existing key moves the new value to the tail, preserving the causal
    /// ordering of unrelated pushes. Without a key, capacity is strict
    /// backpressure and no pending value is evicted.
    pub fn push(
        &self,
        message_type: &str,
        fields: Map<String, Value>,
        conflation_key: Option<&str>,
    ) -> Result<u64, ProviderPushError> {
        let Some(mailbox) = self.mailbox.upgrade() else {
            return Err(ProviderPushError::Closed);
        };
        mailbox.push(&self.domain, message_type, fields, conflation_key)
    }

    /// Validates and enqueues an already-encoded provider envelope.
    pub fn push_envelope(
        &self,
        envelope: &BoundedJson,
        conflation_key: Option<&str>,
    ) -> Result<u64, ProviderPushError> {
        let Value::Object(mut fields) = envelope
            .decode()
            .map_err(|error| ProviderPushError::Malformed(Arc::from(error.to_string())))?
        else {
            return Err(ProviderPushError::Malformed(Arc::from(
                "provider push envelope must be an object",
            )));
        };
        let Some(Value::String(message_type)) = fields.remove("type") else {
            return Err(ProviderPushError::Malformed(Arc::from(
                "provider push envelope requires a string type",
            )));
        };
        self.push(&message_type, fields, conflation_key)
    }

    /// Fail-closes the exact mapped outbound sink. The native sink observes
    /// the typed termination and must tear down the same source-window/session
    /// mapping; no provider can retarget another session.
    pub fn terminate(&self, reason: ProviderPushTermination) {
        if let Some(mailbox) = self.mailbox.upgrade() {
            mailbox.terminate(reason);
        }
    }
}

#[derive(Debug)]
pub struct ProviderPushObserver {
    mailbox: Arc<OutboundMailbox>,
    changed: watch::Receiver<u64>,
}

impl ProviderPushObserver {
    pub fn session(&self) -> SessionId {
        self.mailbox.session
    }

    pub fn source_window(&self) -> SourceWindowId {
        self.mailbox.source_window
    }

    /// Destructively drains at most `maximum` pushes for the one native sink.
    pub fn drain(&self, maximum: usize) -> Result<ProviderPushBatch, ProviderPushError> {
        self.mailbox.drain(maximum)
    }

    /// Event-driven wait for a push or closure. The returned batch is bounded
    /// by `maximum`; callers never poll a queue.
    pub async fn changed(
        &mut self,
        maximum: usize,
    ) -> Result<ProviderPushBatch, ProviderPushError> {
        loop {
            let batch = self.mailbox.drain(maximum)?;
            if !batch.pushes.is_empty() || batch.closed {
                return Ok(batch);
            }
            self.changed
                .changed()
                .await
                .map_err(|_| ProviderPushError::Closed)?;
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ProviderPushError {
    #[error("provider push sink is closed")]
    Closed,
    #[error("provider domain was revoked for this mapped session")]
    Revoked,
    #[error("provider push message type is outside its bound domain")]
    DomainMismatch,
    #[error("provider push contains a reserved runtime-authority field")]
    AuthorityField,
    #[error("provider push envelope is malformed: {0}")]
    Malformed(Arc<str>),
    #[error("provider push envelope is {actual} bytes; maximum is {maximum}")]
    EnvelopeTooLarge { actual: usize, maximum: usize },
    #[error("provider push queue count capacity {capacity} is full")]
    CountCapacity { capacity: usize },
    #[error(
        "provider push queue byte capacity {capacity} cannot admit {requested} additional bytes"
    )]
    ByteCapacity { capacity: usize, requested: usize },
    #[error("provider push conflation key is {actual} bytes; maximum is {maximum}")]
    ConflationKeyTooLarge { actual: usize, maximum: usize },
    #[error("provider push drain maximum must be finite and non-zero")]
    InvalidDrainLimit,
}

#[derive(Debug)]
pub(crate) struct OutboundMailbox {
    session: SessionId,
    source_window: SourceWindowId,
    limits: ProviderPushLimits,
    state: Mutex<OutboundState>,
    closed: AtomicBool,
    changed: watch::Sender<u64>,
}

#[derive(Debug, Default)]
struct OutboundState {
    next_sequence: u64,
    pending_bytes: usize,
    pending: VecDeque<PendingPush>,
    revoked: BTreeSet<Capability>,
    termination: Option<ProviderPushTermination>,
}

#[derive(Debug)]
struct PendingPush {
    push: ProviderPush,
    conflation: Option<(Capability, Arc<str>)>,
}

impl OutboundMailbox {
    pub(crate) fn new(
        _principal: Principal,
        session: SessionId,
        source_window: SourceWindowId,
        limits: ProviderPushLimits,
    ) -> Arc<Self> {
        let (changed, _) = watch::channel(0);
        Arc::new(Self {
            session,
            source_window,
            limits,
            state: Mutex::new(OutboundState::default()),
            closed: AtomicBool::new(false),
            changed,
        })
    }

    pub(crate) fn sender(self: &Arc<Self>, domain: Capability) -> ProviderPushSender {
        ProviderPushSender {
            session: self.session,
            source_window: self.source_window,
            domain,
            mailbox: Arc::downgrade(self),
        }
    }

    pub(crate) fn observe(self: &Arc<Self>) -> ProviderPushObserver {
        ProviderPushObserver {
            mailbox: Arc::clone(self),
            changed: self.changed.subscribe(),
        }
    }

    pub(crate) fn revoke(&self, domain: &Capability) {
        let mut state = self.state.lock();
        state.revoked.insert(domain.clone());
        let mut removed_bytes = 0;
        state.pending.retain(|pending| {
            if &pending.push.domain == domain {
                removed_bytes += pending.push.envelope.byte_len();
                false
            } else {
                true
            }
        });
        state.pending_bytes = state.pending_bytes.saturating_sub(removed_bytes);
        state.next_sequence = state.next_sequence.saturating_add(1);
        self.changed.send_replace(state.next_sequence);
    }

    pub(crate) fn close(&self) {
        self.close_with(None);
    }

    pub(crate) fn terminate(&self, reason: ProviderPushTermination) {
        self.close_with(Some(reason));
    }

    fn close_with(&self, termination: Option<ProviderPushTermination>) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut state = self.state.lock();
        state.termination = termination;
        state.pending.clear();
        state.pending_bytes = 0;
        state.next_sequence = state.next_sequence.saturating_add(1);
        self.changed.send_replace(state.next_sequence);
    }

    fn push(
        &self,
        domain: &Capability,
        message_type: &str,
        fields: Map<String, Value>,
        conflation_key: Option<&str>,
    ) -> Result<u64, ProviderPushError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderPushError::Closed);
        }
        let Some((wire_domain, _)) = message_type.split_once('.') else {
            return Err(ProviderPushError::DomainMismatch);
        };
        if wire_domain != domain.as_str() {
            return Err(ProviderPushError::DomainMismatch);
        }
        if fields
            .keys()
            .any(|field| RESERVED_AUTHORITY_FIELDS.contains(&field.as_str()))
        {
            return Err(ProviderPushError::AuthorityField);
        }
        if fields.contains_key("type") {
            return Err(ProviderPushError::Malformed(Arc::from(
                "provider fields cannot redefine type",
            )));
        }
        let conflation_key = conflation_key
            .map(|key| {
                if key.is_empty() {
                    return Err(ProviderPushError::Malformed(Arc::from(
                        "conflation key cannot be empty",
                    )));
                }
                if key.len() > self.limits.maximum_conflation_key_bytes {
                    return Err(ProviderPushError::ConflationKeyTooLarge {
                        actual: key.len(),
                        maximum: self.limits.maximum_conflation_key_bytes,
                    });
                }
                Ok(Arc::<str>::from(key))
            })
            .transpose()?;
        let mut envelope = fields;
        envelope.insert("type".to_owned(), Value::String(message_type.to_owned()));
        let envelope =
            BoundedJson::from_value(&Value::Object(envelope), self.limits.maximum_envelope_bytes)
                .map_err(|error| match error {
                nmp_native_runtime_core::BoundedJsonError::TooLarge { actual, maximum } => {
                    ProviderPushError::EnvelopeTooLarge { actual, maximum }
                }
                nmp_native_runtime_core::BoundedJsonError::Invalid(reason) => {
                    ProviderPushError::Malformed(Arc::from(reason))
                }
            })?;
        let envelope_bytes = envelope.byte_len();

        let mut state = self.state.lock();
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderPushError::Closed);
        }
        if state.revoked.contains(domain) {
            return Err(ProviderPushError::Revoked);
        }
        let conflation = conflation_key.map(|key| (domain.clone(), key));
        let replaced = conflation.as_ref().and_then(|needle| {
            state
                .pending
                .iter()
                .position(|pending| pending.conflation.as_ref() == Some(needle))
        });
        let replaced_bytes = replaced
            .and_then(|index| state.pending.get(index))
            .map_or(0, |pending| pending.push.envelope.byte_len());
        if replaced.is_none() && state.pending.len() >= self.limits.maximum_pending_count {
            return Err(ProviderPushError::CountCapacity {
                capacity: self.limits.maximum_pending_count,
            });
        }
        let prospective = state
            .pending_bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(envelope_bytes);
        if prospective > self.limits.maximum_pending_bytes {
            return Err(ProviderPushError::ByteCapacity {
                capacity: self.limits.maximum_pending_bytes,
                requested: envelope_bytes.saturating_sub(replaced_bytes),
            });
        }
        if let Some(index) = replaced {
            state.pending.remove(index);
        }
        state.next_sequence = state.next_sequence.saturating_add(1);
        let sequence = state.next_sequence;
        state.pending_bytes = prospective;
        state.pending.push_back(PendingPush {
            push: ProviderPush {
                sequence,
                session: self.session,
                source_window: self.source_window,
                domain: domain.clone(),
                envelope,
            },
            conflation,
        });
        self.changed.send_replace(sequence);
        Ok(sequence)
    }

    fn drain(&self, maximum: usize) -> Result<ProviderPushBatch, ProviderPushError> {
        if maximum == 0 {
            return Err(ProviderPushError::InvalidDrainLimit);
        }
        let mut state = self.state.lock();
        let count = maximum.min(state.pending.len());
        let mut pushes = Vec::with_capacity(count);
        for _ in 0..count {
            let pending = state
                .pending
                .pop_front()
                .expect("count was bounded by the pending queue length");
            state.pending_bytes = state
                .pending_bytes
                .saturating_sub(pending.push.envelope.byte_len());
            pushes.push(pending.push);
        }
        Ok(ProviderPushBatch {
            pushes,
            closed: self.closed.load(Ordering::Acquire),
            termination: state.termination,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal() -> Principal {
        Principal::new("a".repeat(64), "app", "b".repeat(64)).unwrap()
    }

    fn mailbox(count: usize, bytes: usize) -> Arc<OutboundMailbox> {
        OutboundMailbox::new(
            principal(),
            SessionId(7),
            SourceWindowId(11),
            ProviderPushLimits {
                maximum_pending_count: count,
                maximum_pending_bytes: bytes,
                maximum_envelope_bytes: bytes,
                maximum_conflation_key_bytes: 32,
            },
        )
    }

    #[test]
    fn conflation_moves_latest_to_tail_without_reordering_other_keys() {
        let mailbox = mailbox(3, 4_096);
        let sender = mailbox.sender(Capability::new("theme").unwrap());
        sender
            .push(
                "theme.changed",
                Map::from_iter([("value".to_owned(), Value::from(1))]),
                Some("theme"),
            )
            .unwrap();
        sender
            .push(
                "theme.notice",
                Map::from_iter([("value".to_owned(), Value::from(2))]),
                None,
            )
            .unwrap();
        sender
            .push(
                "theme.changed",
                Map::from_iter([("value".to_owned(), Value::from(3))]),
                Some("theme"),
            )
            .unwrap();
        let batch = mailbox.observe().drain(3).unwrap();
        assert_eq!(batch.pushes.len(), 2);
        assert!(batch.pushes[0].envelope.as_str().contains("theme.notice"));
        assert!(batch.pushes[1].envelope.as_str().contains("\"value\":3"));
        assert!(batch.pushes[0].sequence < batch.pushes[1].sequence);
    }

    #[test]
    fn slow_consumer_gets_typed_backpressure_without_eviction() {
        let mailbox = mailbox(1, 4_096);
        let sender = mailbox.sender(Capability::new("identity").unwrap());
        sender.push("identity.changed", Map::new(), None).unwrap();
        assert_eq!(
            sender.push("identity.changed", Map::new(), None),
            Err(ProviderPushError::CountCapacity { capacity: 1 })
        );
        assert_eq!(mailbox.observe().drain(8).unwrap().pushes.len(), 1);
    }

    #[test]
    fn revoke_and_close_fail_closed_and_discard_pending_values() {
        let mailbox = mailbox(4, 4_096);
        let sender = mailbox.sender(Capability::new("config").unwrap());
        sender
            .push("config.changed", Map::new(), Some("config"))
            .unwrap();
        mailbox.revoke(&Capability::new("config").unwrap());
        assert_eq!(
            sender.push("config.changed", Map::new(), Some("config")),
            Err(ProviderPushError::Revoked)
        );
        assert!(mailbox.observe().drain(4).unwrap().pushes.is_empty());
        mailbox.close();
        assert_eq!(
            sender.push("config.changed", Map::new(), Some("config")),
            Err(ProviderPushError::Closed)
        );
    }

    #[test]
    fn provider_cannot_embed_or_retarget_runtime_authority() {
        let mailbox = mailbox(4, 4_096);
        let sender = mailbox.sender(Capability::new("identity").unwrap());
        assert_eq!(
            sender.push(
                "identity.changed",
                Map::from_iter([("sessionId".to_owned(), Value::from(99))]),
                Some("identity"),
            ),
            Err(ProviderPushError::AuthorityField)
        );
        assert_eq!(
            sender.push("theme.changed", Map::new(), Some("identity")),
            Err(ProviderPushError::DomainMismatch)
        );
    }
}
