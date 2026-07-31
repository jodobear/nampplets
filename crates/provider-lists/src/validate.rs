use std::{collections::BTreeSet, sync::Arc};

use serde_json::{Map, Value};

use crate::{
    ListEntry, ListItemTag, ListMutation, ListSelector, ListsProviderLimits, request::ListRefusal,
};

pub(crate) fn validate_value(
    tag: ListItemTag,
    value: &str,
    limits: ListsProviderLimits,
) -> Result<(), ListRefusal> {
    let reject = |expectation: &str| ListRefusal::ItemValueRejected {
        tag,
        expectation: Arc::from(expectation),
    };
    match tag {
        ListItemTag::P | ListItemTag::E => {
            if !is_hex32(value) {
                return Err(reject("64 lowercase hex characters"));
            }
        }
        ListItemTag::T => {
            if value.is_empty()
                || value.len() > limits.maximum_value_bytes
                || value.chars().any(char::is_whitespace)
                || value.starts_with('#')
            {
                return Err(reject(
                    "a non-empty hashtag without whitespace or a leading #",
                ));
            }
        }
        ListItemTag::A => {
            if value.len() > limits.maximum_value_bytes || !is_address(value) {
                return Err(reject("kind:pubkey:identifier"));
            }
        }
    }
    Ok(())
}

/// Lowercase-only: NMP addresses events and keys by exact lowercase hex, so
/// accepting mixed case here would let the same entry appear twice.
fn is_hex32(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_address(value: &str) -> bool {
    let mut parts = value.splitn(3, ':');
    let (Some(kind), Some(pubkey), Some(identifier)) = (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    !kind.is_empty()
        && kind.len() <= 5
        && kind.bytes().all(|byte| byte.is_ascii_digit())
        && is_hex32(pubkey)
        && !identifier.contains(':')
}

/// Decides the exact result of adding `items` to `current`.
///
/// Order is preserved and additions append, so an unrelated reordering never
/// leaks out of a mutation.
pub(crate) fn apply_add(
    current: &[ListEntry],
    items: &[ListEntry],
    limits: ListsProviderLimits,
) -> Result<ListMutation, ListRefusal> {
    let present = current.iter().cloned().collect::<BTreeSet<_>>();
    let mut entries = current.to_vec();
    let mut changed = 0;
    let mut skipped = 0;
    for item in items {
        if present.contains(item) {
            skipped += 1;
        } else {
            entries.push(item.clone());
            changed += 1;
        }
    }
    if entries.len() > limits.maximum_list_entries {
        return Err(ListRefusal::ListFull(limits.maximum_list_entries));
    }
    Ok(ListMutation {
        entries,
        changed,
        skipped,
    })
}

/// Decides the exact result of removing `items` from `current`.
pub(crate) fn apply_remove(current: &[ListEntry], items: &[ListEntry]) -> ListMutation {
    let removing = items.iter().cloned().collect::<BTreeSet<_>>();
    let entries = current
        .iter()
        .filter(|entry| !removing.contains(*entry))
        .cloned()
        .collect::<Vec<_>>();
    let present = current.iter().cloned().collect::<BTreeSet<_>>();
    let changed = items.iter().filter(|item| present.contains(*item)).count();
    ListMutation {
        entries,
        changed,
        skipped: items.len() - changed,
    }
}

pub(crate) fn validate_limits(limits: ListsProviderLimits) -> bool {
    ![
        limits.maximum_sessions,
        limits.maximum_response_bytes,
        limits.maximum_draft_bytes,
        limits.maximum_correlation_id_bytes,
        limits.maximum_request_items,
        limits.maximum_list_entries,
        limits.maximum_identifier_bytes,
        limits.maximum_value_bytes,
    ]
    .contains(&0)
}

pub(crate) fn selector_value(selector: &ListSelector) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("kind".to_owned(), Value::from(selector.kind));
    if let Some(identifier) = &selector.identifier {
        map.insert(
            "identifier".to_owned(),
            Value::from(identifier.as_ref().to_owned()),
        );
    }
    map
}
