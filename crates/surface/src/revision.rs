use nmp_native_runtime_core::BoundedJson;
use thiserror::Error;

use crate::BindingSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceFrame {
    Snapshot {
        revision: u64,
        value: BoundedJson,
        scoped_evidence: BoundedJson,
    },
    Delta {
        from_revision: u64,
        revision: u64,
        value: BoundedJson,
    },
    Resync,
}

impl From<&BindingSnapshot> for SurfaceFrame {
    fn from(snapshot: &BindingSnapshot) -> Self {
        Self::Snapshot {
            revision: snapshot.revision,
            value: snapshot.value.clone(),
            scoped_evidence: snapshot.scoped_evidence.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SurfaceClientProjection {
    revision: Option<u64>,
    value: Option<BoundedJson>,
}

impl SurfaceClientProjection {
    pub fn revision(&self) -> Option<u64> {
        self.revision
    }

    pub fn apply(&mut self, frame: SurfaceFrame) -> Result<ApplyOutcome, SurfaceProjectionError> {
        match frame {
            SurfaceFrame::Snapshot {
                revision, value, ..
            } => {
                self.revision = Some(revision);
                self.value = Some(value);
                Ok(ApplyOutcome::Applied { revision })
            }
            SurfaceFrame::Delta {
                from_revision,
                revision,
                value,
            } => {
                if revision <= from_revision {
                    return Err(SurfaceProjectionError::InvalidDelta {
                        from_revision,
                        revision,
                    });
                }
                if self.revision != Some(from_revision) {
                    return Ok(ApplyOutcome::RequestSnapshot {
                        current_revision: self.revision,
                        rejected_from_revision: from_revision,
                    });
                }
                self.revision = Some(revision);
                self.value = Some(value);
                Ok(ApplyOutcome::Applied { revision })
            }
            SurfaceFrame::Resync => {
                self.revision = None;
                self.value = None;
                Ok(ApplyOutcome::AwaitingSnapshot)
            }
        }
    }

    pub fn authoritative_resync(
        &mut self,
        latest: &BindingSnapshot,
    ) -> Result<ApplyOutcome, SurfaceProjectionError> {
        self.apply(SurfaceFrame::from(latest))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied {
        revision: u64,
    },
    RequestSnapshot {
        current_revision: Option<u64>,
        rejected_from_revision: u64,
    },
    AwaitingSnapshot,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SurfaceProjectionError {
    #[error("delta revision {revision} must be greater than fromRevision {from_revision}")]
    InvalidDelta { from_revision: u64, revision: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(value: serde_json::Value) -> BoundedJson {
        BoundedJson::from_value(&value, 1024).unwrap()
    }

    #[test]
    fn gap_requests_resync_without_applying_delta() {
        let evidence = json(serde_json::json!({}));
        let mut projection = SurfaceClientProjection::default();
        projection
            .apply(SurfaceFrame::Snapshot {
                revision: 10,
                value: json(serde_json::json!({"value": 10})),
                scoped_evidence: evidence.clone(),
            })
            .unwrap();
        assert_eq!(
            projection
                .apply(SurfaceFrame::Delta {
                    from_revision: 11,
                    revision: 12,
                    value: json(serde_json::json!({"value": 12})),
                })
                .unwrap(),
            ApplyOutcome::RequestSnapshot {
                current_revision: Some(10),
                rejected_from_revision: 11,
            }
        );
        assert_eq!(projection.revision(), Some(10));
        let latest = BindingSnapshot {
            revision: 12,
            value: json(serde_json::json!({"value": 12})),
            scoped_evidence: evidence,
        };
        projection.authoritative_resync(&latest).unwrap();
        assert_eq!(projection.revision(), Some(12));
    }
}
