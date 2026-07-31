//! NAP-INTENT archetype declarations parsed from `["archetype", slug,
//! protocol, ...]` manifest tags. A napplet declares one such tag per
//! archetype/protocol pair it can act as a handler for; the runtime uses
//! these to register the napplet as an `IntentProvider` handler on install.

use std::sync::Arc;

use crate::ManifestError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchetypeDeclaration {
    pub slug: Arc<str>,
    pub protocol: Arc<str>,
}

pub(crate) fn parse_archetype_tag(
    fields: &[String],
) -> Result<ArchetypeDeclaration, ManifestError> {
    if fields.len() < 3 {
        return Err(ManifestError::MalformedCriticalTag {
            name: fields.first().cloned().unwrap_or_default(),
            expected: 3,
            actual: fields.len(),
        });
    }
    let slug = &fields[1];
    if !valid_slug(slug) {
        return Err(ManifestError::InvalidArchetypeSlug(slug.clone()));
    }
    let protocol = &fields[2];
    if !valid_protocol(protocol) {
        return Err(ManifestError::InvalidArchetypeProtocol(protocol.clone()));
    }
    if fields[3..].iter().any(|kind| !valid_kind(kind)) {
        return Err(ManifestError::InvalidArchetypeKind(fields[3..].join(",")));
    }
    Ok(ArchetypeDeclaration {
        slug: Arc::from(slug.as_str()),
        protocol: Arc::from(protocol.as_str()),
    })
}

/// Mirrors `provider-link::intent`'s `valid_slug` predicate exactly, so a
/// slug that parses here is guaranteed to also pass `IntentProvider`'s own
/// `register_handler` validation. `provider-link` isn't (and shouldn't
/// become) a dependency of this crate, so the rule is duplicated rather than
/// shared.
fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

/// Mirrors `provider-link::intent`'s queryless convention rule exactly, so a
/// manifest declaration cannot pass artifact verification and then disappear
/// when the intent provider registers the installed handler.
fn valid_protocol(value: &str) -> bool {
    let Some(path) = value.strip_prefix("napplet:") else {
        return false;
    };
    let Some((subject, action)) = path.split_once('/') else {
        return false;
    };
    !value.is_empty()
        && value.len() <= 1_024
        && !value.bytes().any(|byte| byte.is_ascii_control())
        && !subject.is_empty()
        && !action.is_empty()
        && !action.contains('/')
        && [subject, action].into_iter().all(|component| {
            !component
                .chars()
                .any(|character| character.is_whitespace() || matches!(character, '?' | '#'))
        })
}

fn valid_kind(value: &str) -> bool {
    value.strip_prefix("kind:").is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(fields: &[&str]) -> Vec<String> {
        fields.iter().map(|field| field.to_string()).collect()
    }

    #[test]
    fn valid_archetype_tag_parses_slug_and_protocol() {
        let declaration = parse_archetype_tag(&tag(&[
            "archetype",
            "nip29-group",
            "napplet:nip29-group/open",
        ]))
        .unwrap();
        assert_eq!(declaration.slug.as_ref(), "nip29-group");
        assert_eq!(declaration.protocol.as_ref(), "napplet:nip29-group/open");
    }

    #[test]
    fn valid_archetype_tag_accepts_trailing_kind_hints() {
        let declaration = parse_archetype_tag(&tag(&[
            "archetype",
            "nip29-group",
            "napplet:nip29-group/open",
            "kind:39000",
            "kind:39001",
        ]))
        .unwrap();
        assert_eq!(declaration.slug.as_ref(), "nip29-group");
    }

    #[test]
    fn archetype_tag_rejects_too_few_fields() {
        assert!(parse_archetype_tag(&tag(&["archetype", "nip29-group"])).is_err());
    }

    #[test]
    fn archetype_tag_rejects_protocol_outside_the_napplet_namespace() {
        assert!(parse_archetype_tag(&tag(&["archetype", "nip29-group", "https://evil"])).is_err());
    }

    #[test]
    fn archetype_tag_rejects_protocol_the_intent_provider_cannot_register() {
        let invalid = [
            "napplet:nip29-group".to_owned(),
            "napplet:/open".to_owned(),
            "napplet:nip29-group/".to_owned(),
            "napplet:nip29-group/open/extra".to_owned(),
            "napplet:nip29-group/open?mode=compact".to_owned(),
            "napplet:nip29-group/open#fragment".to_owned(),
            "napplet:nip29-group/\0open".to_owned(),
            format!("napplet:nip29-group/{}", "ö".repeat(512)),
        ];
        for protocol in invalid {
            let fields = ["archetype".to_owned(), "nip29-group".to_owned(), protocol];
            assert!(parse_archetype_tag(&fields).is_err());
        }
    }

    #[test]
    fn archetype_tag_rejects_malformed_kind_hint() {
        assert!(
            parse_archetype_tag(&tag(&[
                "archetype",
                "nip29-group",
                "napplet:nip29-group/open",
                "kind:not-a-number"
            ]))
            .is_err()
        );
    }
}
