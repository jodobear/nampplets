use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub(crate) const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityProviderLimits {
    pub maximum_sessions: usize,
    pub maximum_response_bytes: usize,
    pub maximum_evidence_bytes: usize,
    pub maximum_correlation_id_bytes: usize,
    pub maximum_list_type_bytes: usize,
    pub maximum_items: usize,
    pub maximum_relays: usize,
    pub maximum_text_bytes: usize,
    pub maximum_thumbnails_per_badge: usize,
}

impl Default for IdentityProviderLimits {
    fn default() -> Self {
        Self {
            maximum_sessions: 64,
            maximum_response_bytes: 512 * 1024,
            maximum_evidence_bytes: 128 * 1024,
            maximum_correlation_id_bytes: 1_024,
            maximum_list_type_bytes: 128,
            maximum_items: 1_024,
            maximum_relays: 256,
            maximum_text_bytes: 16 * 1024,
            maximum_thumbnails_per_badge: 32,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nip05: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lud16: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayPermission {
    pub read: bool,
    pub write: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ZapReceipt {
    pub event_id: String,
    pub sender: String,
    pub amount: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Badge {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbs: Option<Vec<String>>,
    pub awarded_by: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityValue {
    Relays(BTreeMap<String, RelayPermission>),
    Profile(Option<ProfileData>),
    Follows(Vec<String>),
    List(Vec<String>),
    Zaps(Vec<ZapReceipt>),
    Mutes(Vec<String>),
    Blocked(Vec<String>),
    Badges(Vec<Badge>),
}
