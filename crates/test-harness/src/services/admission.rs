use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceCensus {
    pub active: usize,
    pub high_watermark: usize,
    pub refusals: u64,
}

pub(super) fn admit(
    active: &AtomicUsize,
    high_watermark: &AtomicUsize,
    refusals: &AtomicU64,
    capacity: usize,
    resource: &'static str,
) -> Result<(), DeterministicServiceError> {
    let mut current = active.load(Ordering::Acquire);
    loop {
        if current >= capacity {
            refusals.fetch_add(1, Ordering::AcqRel);
            return Err(DeterministicServiceError::Capacity { resource, capacity });
        }
        match active.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                high_watermark.fetch_max(current + 1, Ordering::AcqRel);
                return Ok(());
            }
            Err(updated) => current = updated,
        }
    }
}

pub(super) fn validate_fixture_name(name: &str) -> Result<(), DeterministicServiceError> {
    let path = Path::new(name);
    if path.is_absolute()
        || name.contains('\\')
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(DeterministicServiceError::InvalidFixtureName(
            name.to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum DeterministicServiceError {
    #[error("invalid scenario catalog: {0}")]
    InvalidCatalog(String),
    #[error("unknown scenario {0}")]
    UnknownScenario(String),
    #[error("invalid scripted step {0}")]
    InvalidStep(String),
    #[error("script contains {actual} steps; the maximum is {maximum}")]
    ScriptTooLong { actual: usize, maximum: usize },
    #[error("{resource} capacity {capacity} is full")]
    Capacity {
        resource: &'static str,
        capacity: usize,
    },
    #[error("fixture frame is {actual} bytes; the maximum is {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },
    #[error("invalid fixture name {0}")]
    InvalidFixtureName(String),
    #[error("fixture I/O failed at {path}: {source}")]
    FixtureIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("signer expected request {expected}, got {actual}")]
    UnexpectedRequest { expected: String, actual: String },
}
