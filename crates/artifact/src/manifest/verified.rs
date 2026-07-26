use std::sync::Arc;

use super::ArtifactMode;
use crate::{ArchetypeDeclaration, ArtifactManifest, Sha256Digest};

#[derive(Clone, Debug)]
pub struct VerifiedManifest {
    pub(super) event_id: Sha256Digest,
    pub(super) author: Sha256Digest,
    pub(super) kind: u16,
    pub(super) d_tag: Option<Arc<str>>,
    pub(super) aggregate: Sha256Digest,
    pub(super) signed_event_json: Arc<[u8]>,
    pub(super) artifact: ArtifactManifest,
    pub(super) mode: ArtifactMode,
    pub(super) requirements: Arc<[Arc<str>]>,
    pub(super) servers: Arc<[Arc<str>]>,
    pub(super) archetypes: Arc<[ArchetypeDeclaration]>,
    pub(super) title: Option<Arc<str>>,
    pub(super) description: Option<Arc<str>>,
    pub(super) source: Option<Arc<str>>,
}

impl VerifiedManifest {
    pub fn event_id(&self) -> &Sha256Digest {
        &self.event_id
    }

    pub fn author(&self) -> &Sha256Digest {
        &self.author
    }

    pub fn kind(&self) -> u16 {
        self.kind
    }

    pub fn d_tag(&self) -> Option<&str> {
        self.d_tag.as_deref()
    }

    pub fn aggregate(&self) -> &Sha256Digest {
        &self.aggregate
    }

    /// The exact signed event bytes this manifest was verified from, in
    /// canonical NIP-01 JSON. Retained so a caller can persist enough to
    /// re-verify and reopen this exact build later without a second network
    /// fetch of an event that may since have been superseded by a
    /// republished `d` tag.
    pub fn signed_event_json(&self) -> &[u8] {
        &self.signed_event_json
    }

    pub fn mode(&self) -> ArtifactMode {
        self.mode
    }

    pub fn requirements(&self) -> impl ExactSizeIterator<Item = &str> {
        self.requirements.iter().map(AsRef::as_ref)
    }

    pub fn servers(&self) -> impl ExactSizeIterator<Item = &str> {
        self.servers.iter().map(AsRef::as_ref)
    }

    pub fn archetypes(&self) -> impl ExactSizeIterator<Item = &ArchetypeDeclaration> {
        self.archetypes.iter()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
}
