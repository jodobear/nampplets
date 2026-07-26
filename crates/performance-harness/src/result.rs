//! V1 result and observed-only comparison artifacts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    AttemptOutcome, Environment, HarnessError, ProducerSummary, RESULT_SCHEMA_ID,
    statistics::summarize,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Cold,
    Warm,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureIdentity {
    pub id: String,
    pub sha256: String,
    pub cardinality: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protocol {
    pub warmup_count: usize,
    pub sample_count: usize,
    pub per_sample_deadline_ns: u64,
    pub run_deadline_ns: u64,
    pub outlier_policy: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceIdentity {
    pub benchmark_id: String,
    pub state: RunState,
    pub reset_scopes: Vec<String>,
    pub fixture: FixtureIdentity,
    pub protocol: Protocol,
    pub build_mode: String,
    pub toolchain: String,
    #[serde(flatten)]
    pub environment: Environment,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub source_revision: String,
    pub artifact_locator: String,
    pub source_provenance: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub domain: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub code: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Sample {
    Success {
        sequence: usize,
        duration_ns: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        cpu_time_ns: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        peak_rss_bytes: Option<u64>,
    },
    Refused {
        sequence: usize,
        duration_ns: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        cpu_time_ns: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        peak_rss_bytes: Option<u64>,
        refusal: Refusal,
    },
    Failed {
        sequence: usize,
        duration_ns: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        cpu_time_ns: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        peak_rss_bytes: Option<u64>,
        failure: Failure,
    },
    DeadlineExceeded {
        sequence: usize,
        duration_ns: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        cpu_time_ns: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        peak_rss_bytes: Option<u64>,
    },
}

impl Sample {
    pub(crate) fn from_attempt(sequence: usize, duration_ns: u64, outcome: AttemptOutcome) -> Self {
        match outcome {
            AttemptOutcome::Success => Self::Success {
                sequence,
                duration_ns,
                cpu_time_ns: None,
                peak_rss_bytes: None,
            },
            AttemptOutcome::Refused(refusal) => Self::Refused {
                sequence,
                duration_ns,
                cpu_time_ns: None,
                peak_rss_bytes: None,
                refusal,
            },
            AttemptOutcome::Failed(failure) => Self::Failed {
                sequence,
                duration_ns,
                cpu_time_ns: None,
                peak_rss_bytes: None,
                failure,
            },
        }
    }

    pub fn sequence(&self) -> usize {
        match self {
            Self::Success { sequence, .. }
            | Self::Refused { sequence, .. }
            | Self::Failed { sequence, .. }
            | Self::DeadlineExceeded { sequence, .. } => *sequence,
        }
    }

    pub fn duration_ns(&self) -> u64 {
        match self {
            Self::Success { duration_ns, .. }
            | Self::Refused { duration_ns, .. }
            | Self::Failed { duration_ns, .. }
            | Self::DeadlineExceeded { duration_ns, .. } => *duration_ns,
        }
    }

    pub(crate) fn outcome(&self) -> &'static str {
        match self {
            Self::Success { .. } => "success",
            Self::Refused { .. } => "refused",
            Self::Failed { .. } => "failed",
            Self::DeadlineExceeded { .. } => "deadline_exceeded",
        }
    }

    pub(crate) fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Refused { refusal, .. } => Some(refusal),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultArtifact {
    pub schema_id: String,
    pub run_id: String,
    pub identity: EvidenceIdentity,
    pub build: BuildIdentity,
    pub samples: Vec<Sample>,
    pub producer_summary: ProducerSummary,
    pub comparison_key: String,
    pub checksum_sha256: String,
}

impl ResultArtifact {
    pub fn new(
        run_id: impl Into<String>,
        identity: EvidenceIdentity,
        build: BuildIdentity,
        samples: Vec<Sample>,
    ) -> Result<Self, HarnessError> {
        if samples.len() != identity.protocol.sample_count
            || samples
                .iter()
                .enumerate()
                .any(|(index, sample)| sample.sequence() != index)
        {
            return Err(HarnessError::InvalidResult("sample identity mismatch"));
        }
        if samples
            .iter()
            .any(|sample| sample.duration_ns() > crate::MAX_SAMPLE_DEADLINE_NS)
        {
            return Err(HarnessError::InvalidResult("sample duration exceeds v1"));
        }
        let producer_summary = summarize(&samples)?;
        let comparison_key = digest_value(&identity)?;
        let mut artifact = Self {
            schema_id: RESULT_SCHEMA_ID.to_owned(),
            run_id: run_id.into(),
            identity,
            build,
            samples,
            producer_summary,
            comparison_key,
            checksum_sha256: String::new(),
        };
        artifact.checksum_sha256 = artifact_checksum(&artifact)?;
        Ok(artifact)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, HarnessError> {
        canonical_value(self)
    }

    pub(crate) fn reference(&self) -> Result<ResultReference, HarnessError> {
        Ok(ResultReference {
            artifact_locator: self.build.artifact_locator.clone(),
            source_revision: self.build.source_revision.clone(),
            source_provenance: self.build.source_provenance.clone(),
            checksum_sha256: self.checksum_sha256.clone(),
            comparison_key: self.comparison_key.clone(),
            identity: self.identity.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResultReference {
    pub(crate) artifact_locator: String,
    pub(crate) source_revision: String,
    pub(crate) source_provenance: String,
    pub(crate) checksum_sha256: String,
    pub(crate) comparison_key: String,
    pub(crate) identity: EvidenceIdentity,
}

pub(crate) fn canonical_value(value: &impl Serialize) -> Result<Vec<u8>, HarnessError> {
    let value = serde_json::to_value(value)
        .map_err(|error| HarnessError::Serialization(error.to_string()))?;
    serde_json::to_vec(&value).map_err(|error| HarnessError::Serialization(error.to_string()))
}

fn digest_value(value: &impl Serialize) -> Result<String, HarnessError> {
    Ok(format!("{:x}", Sha256::digest(canonical_value(value)?)))
}

pub(crate) fn artifact_checksum(value: &impl Serialize) -> Result<String, HarnessError> {
    let mut value = serde_json::to_value(value)
        .map_err(|error| HarnessError::Serialization(error.to_string()))?;
    let Value::Object(fields) = &mut value else {
        return Err(HarnessError::InvalidResult(
            "artifact root is not an object",
        ));
    };
    fields.remove("checksum_sha256");
    Ok(format!("{:x}", Sha256::digest(canonical_value(&value)?)))
}
