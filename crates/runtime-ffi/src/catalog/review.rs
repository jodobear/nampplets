//! Bounded one-shot exact reviews and their verified-artifact confirmation.

use std::{
    sync::{Arc, mpsc},
    thread,
};

use nmp_native_artifact::ManifestCoordinate;

use super::{
    MAXIMUM_PENDING_REVIEWS, RuntimeCatalogService, StoredReview,
    admission::ActiveCancellation,
    install_eligibility::project_review,
    projection::{duration_millis, map_resolve_error},
    types::{
        RuntimeCatalogConfirmation, RuntimeCatalogConfirmedArtifact, RuntimeCatalogError,
        RuntimeCatalogReview,
    },
};
use nmp_native_catalog_resolver::CancellationToken;

impl RuntimeCatalogService {
    pub fn begin_review_for_entry(
        &self,
        event_id: &str,
    ) -> Result<RuntimeCatalogReview, RuntimeCatalogError> {
        let coordinate = self
            .feed_state
            .lock()
            .candidates
            .get(event_id)
            .cloned()
            .ok_or_else(|| RuntimeCatalogError::InvalidCoordinate {
                reason: "catalog entry is stale or outside the current page".to_owned(),
            })?;
        self.begin_review(coordinate)
    }

    pub fn begin_review(
        &self,
        coordinate: ManifestCoordinate,
    ) -> Result<RuntimeCatalogReview, RuntimeCatalogError> {
        {
            let state = self.reviews.lock();
            if state.reviews.len() >= MAXIMUM_PENDING_REVIEWS {
                return Err(RuntimeCatalogError::ReviewCapacity {
                    maximum: MAXIMUM_PENDING_REVIEWS as u64,
                });
            }
        }
        let permit = self.admission.reserve()?;
        let resolver = Arc::clone(&self.resolver);
        let cancellation = CancellationToken::default();
        let operation_id =
            self.register_operation(ActiveCancellation::Resolve(cancellation.clone()))?;
        let worker_cancellation = cancellation.clone();
        let worker_coordinate = coordinate.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("runtime-catalog-review".to_owned())
            .spawn(move || {
                let _permit = permit;
                let result = resolver.resolve(&worker_coordinate, &worker_cancellation);
                let _ = sender.send(result);
            })
            .map_err(|error| {
                self.remove_operation(operation_id);
                RuntimeCatalogError::WorkerUnavailable {
                    reason: error.to_string(),
                }
            })?;
        let result = match receiver.recv_timeout(self.deadline) {
            Ok(result) => result.map_err(map_resolve_error),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cancellation.cancel();
                Err(RuntimeCatalogError::Deadline {
                    milliseconds: duration_millis(self.deadline),
                })
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(RuntimeCatalogError::WorkerUnavailable {
                    reason: "catalog review worker ended without a result".to_owned(),
                })
            }
        };
        self.remove_operation(operation_id);
        if !matches!(&result, Err(RuntimeCatalogError::Deadline { .. })) {
            let _ = worker.join();
        }
        let review = result?;
        let mut state = self.reviews.lock();
        if state.reviews.len() >= MAXIMUM_PENDING_REVIEWS {
            return Err(RuntimeCatalogError::ReviewCapacity {
                maximum: MAXIMUM_PENDING_REVIEWS as u64,
            });
        }
        state.next_token =
            state
                .next_token
                .checked_add(1)
                .ok_or_else(|| RuntimeCatalogError::Resolve {
                    reason: "catalog review token space is exhausted".to_owned(),
                })?;
        let token = format!("catalog-review-{}", state.next_token);
        let projection =
            project_review(&token, &coordinate, review.handle(), review.lookup_facts())?;
        state.reviews.insert(
            token,
            StoredReview {
                handle: review.handle().clone(),
                projection: projection.clone(),
            },
        );
        Ok(projection)
    }

    pub fn cancel_review(&self, token: &str) -> Result<(), RuntimeCatalogError> {
        self.reviews
            .lock()
            .reviews
            .remove(token)
            .ok_or(RuntimeCatalogError::StaleReview)?;
        Ok(())
    }

    pub fn confirm_review(
        &self,
        token: &str,
    ) -> Result<RuntimeCatalogConfirmedArtifact, RuntimeCatalogError> {
        let stored = self
            .reviews
            .lock()
            .reviews
            .remove(token)
            .ok_or(RuntimeCatalogError::StaleReview)?;
        Ok(RuntimeCatalogConfirmedArtifact {
            confirmation: RuntimeCatalogConfirmation {
                event_id: stored.projection.event_id,
                coordinate: stored.projection.coordinate,
                manifest_author: stored.projection.manifest_author,
                d_tag: stored.projection.d_tag,
                title: stored.projection.title,
                aggregate_hash: stored.projection.aggregate_hash,
                capabilities: stored.projection.capabilities,
                provenance: stored.projection.provenance,
            },
            handle: stored.handle,
        })
    }
}
