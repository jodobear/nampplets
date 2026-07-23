use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_D_TAG_BYTES: usize = 256;

/// Exact executable identity used for grants, storage, and diagnostics.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Principal {
    manifest_author: String,
    d_tag: String,
    aggregate_hash: String,
}

impl Principal {
    pub fn new(
        manifest_author: impl Into<String>,
        d_tag: impl Into<String>,
        aggregate_hash: impl Into<String>,
    ) -> Result<Self, PrincipalError> {
        let manifest_author = manifest_author.into();
        let d_tag = d_tag.into();
        let aggregate_hash = aggregate_hash.into();

        validate_hex_digest("manifest_author", &manifest_author)?;
        validate_hex_digest("aggregate_hash", &aggregate_hash)?;
        if d_tag.is_empty() {
            return Err(PrincipalError::EmptyDTag);
        }
        if d_tag.len() > MAX_D_TAG_BYTES {
            return Err(PrincipalError::DTagTooLong {
                actual: d_tag.len(),
                maximum: MAX_D_TAG_BYTES,
            });
        }

        Ok(Self {
            manifest_author,
            d_tag,
            aggregate_hash,
        })
    }

    pub fn manifest_author(&self) -> &str {
        &self.manifest_author
    }

    pub fn d_tag(&self) -> &str {
        &self.d_tag
    }

    pub fn aggregate_hash(&self) -> &str {
        &self.aggregate_hash
    }
}

impl fmt::Debug for Principal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Principal")
            .field("manifest_author", &self.manifest_author)
            .field("d_tag", &self.d_tag)
            .field("aggregate_hash", &self.aggregate_hash)
            .finish()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PrincipalError {
    #[error("{field} must be exactly 32 bytes encoded as lowercase hex")]
    InvalidHexDigest { field: &'static str },
    #[error("dTag cannot be empty")]
    EmptyDTag,
    #[error("dTag is {actual} bytes; the maximum is {maximum}")]
    DTagTooLong { actual: usize, maximum: usize },
}

fn validate_hex_digest(field: &'static str, value: &str) -> Result<(), PrincipalError> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        || hex::decode(value).map_or(true, |decoded| decoded.len() != 32)
    {
        return Err(PrincipalError::InvalidHexDigest { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_hash_is_part_of_identity() {
        let author = "a".repeat(64);
        let first = Principal::new(&author, "feed", "b".repeat(64)).unwrap();
        let second = Principal::new(&author, "feed", "c".repeat(64)).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn uppercase_or_malformed_digest_is_rejected() {
        assert_eq!(
            Principal::new("A".repeat(64), "feed", "b".repeat(64)),
            Err(PrincipalError::InvalidHexDigest {
                field: "manifest_author"
            })
        );
    }
}
