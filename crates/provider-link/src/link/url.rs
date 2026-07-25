use std::{
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

use serde_json::Value;
use url::{Host, Url};

use super::wire::valid_text;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinkUrlRefusal {
    InvalidUrl,
    UnsupportedScheme,
    BlockedByPolicy,
}

impl LinkUrlRefusal {
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid-url",
            Self::UnsupportedScheme => "unsupported-scheme",
            Self::BlockedByPolicy => "blocked-by-policy",
        }
    }
}

pub(super) fn validate_external_url(
    value: &str,
    maximum_bytes: usize,
) -> Result<Arc<str>, LinkUrlRefusal> {
    if !valid_text(value, maximum_bytes) {
        return Err(LinkUrlRefusal::InvalidUrl);
    }
    let parsed = Url::parse(value).map_err(|_| LinkUrlRefusal::InvalidUrl)?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err(LinkUrlRefusal::UnsupportedScheme);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(LinkUrlRefusal::BlockedByPolicy);
    }
    let host = parsed.host().ok_or(LinkUrlRefusal::InvalidUrl)?;
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if domain == "localhost"
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
                || !domain.contains('.')
            {
                return Err(LinkUrlRefusal::BlockedByPolicy);
            }
        }
        Host::Ipv4(address) if !public_ipv4(address) => {
            return Err(LinkUrlRefusal::BlockedByPolicy);
        }
        Host::Ipv6(address) if !public_ipv6(address) => {
            return Err(LinkUrlRefusal::BlockedByPolicy);
        }
        Host::Ipv4(_) | Host::Ipv6(_) => {}
    }
    let normalized = parsed.to_string();
    if normalized.len() > maximum_bytes {
        return Err(LinkUrlRefusal::InvalidUrl);
    }
    Ok(Arc::from(normalized))
}

pub(super) fn parse_label(
    options: Option<&Value>,
    maximum_label_bytes: usize,
) -> Result<Option<Arc<str>>, &'static str> {
    let Some(options) = options else {
        return Ok(None);
    };
    let object = options
        .as_object()
        .ok_or("`options` must be an object when present")?;
    if object.keys().any(|key| key != "label") {
        return Err("`options` may contain only `label`");
    }
    let Some(label) = object.get("label") else {
        return Ok(None);
    };
    let label = label.as_str().ok_or("`options.label` must be a string")?;
    if label.len() > maximum_label_bytes {
        return Err("`options.label` exceeds the configured byte limit");
    }
    Ok(Some(Arc::from(label)))
}

fn public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, _, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
        || a >= 224)
}

fn public_ipv6(address: Ipv6Addr) -> bool {
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || (address.segments()[0] & 0xfe00) == 0xfc00
        || (address.segments()[0] & 0xffc0) == 0xfe80
        || matches!(address.to_ipv4_mapped(), Some(ipv4) if !public_ipv4(ipv4)))
}
