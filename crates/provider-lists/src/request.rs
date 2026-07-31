use std::{collections::BTreeSet, sync::Arc};

use serde_json::Value;
use thiserror::Error;

use crate::{
    ListEntry, ListItemTag, ListSelector, ListsProviderLimits, SupportedList,
    catalog::{parse_semantic_item_type, supported_list, supported_list_type},
    validate::validate_value,
    write::ListsAction,
};

/// A refusal the napplet sees as `ok: false` with an exact reason.
///
/// These are protocol outcomes, not transport faults: the request was
/// well-formed enough to answer, and the answer is no.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ListRefusal {
    #[error("list must contain exactly one of kind or type")]
    MalformedSelector,
    #[error("this runtime does not service list kind {0}")]
    UnsupportedKind(u16),
    #[error("this runtime does not service list type {0}")]
    UnsupportedType(Arc<str>),
    #[error("list kind {0} is addressed by a d identifier, which is missing")]
    IdentifierRequired(u16),
    #[error("list kind {0} takes no d identifier")]
    IdentifierNotAllowed(u16),
    #[error("identifier must be 1..={0} bytes")]
    IdentifierBounds(usize),
    #[error("items must be a non-empty array of {{itemType, value}} objects")]
    MalformedItems,
    #[error("items must hold 1..={0} entries")]
    ItemBounds(usize),
    #[error("list kind {kind} does not accept {tag} items")]
    ItemTypeRejected { kind: u16, tag: ListItemTag },
    #[error("this runtime cannot encode item type {0}")]
    UnsupportedItemType(Arc<str>),
    #[error("relay and label item hints are not supported by this runtime")]
    ItemHintsUnsupported,
    #[error("private list items are not supported by this runtime")]
    PrivateItemsUnsupported,
    #[error("a {tag} value must be {expectation}")]
    ItemValueRejected {
        tag: ListItemTag,
        expectation: Arc<str>,
    },
    #[error("the same item appears more than once in one request")]
    DuplicateItem,
    #[error("the resulting list would exceed its {0}-entry bound")]
    ListFull(usize),
    #[error("list options must contain only typed create/title/description/image fields")]
    MalformedOptions,
    #[error("list title, description, and image options are not supported by this runtime")]
    MetadataOptionsUnsupported,
    #[error("the requested list does not exist and create is false")]
    ListNotFound,
    #[error("private list items cannot be safely removed without decrypting the list")]
    DecryptFailed,
    #[error("no account is connected, so there is no list to change")]
    NoAccount,
}

impl ListRefusal {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::MalformedSelector | Self::IdentifierNotAllowed(_) | Self::IdentifierBounds(_) => {
                "invalid-list-ref"
            }
            Self::UnsupportedKind(_) | Self::UnsupportedType(_) => "unsupported-list",
            Self::IdentifierRequired(_) => "missing-identifier",
            Self::ItemTypeRejected { .. }
            | Self::UnsupportedItemType(_)
            | Self::ItemHintsUnsupported => "unsupported-item",
            Self::PrivateItemsUnsupported => "private-items-unsupported",
            Self::MalformedItems
            | Self::ItemBounds(_)
            | Self::ItemValueRejected { .. }
            | Self::DuplicateItem
            | Self::ListFull(_) => "invalid-item",
            Self::MalformedOptions | Self::MetadataOptionsUnsupported => "unsupported",
            Self::ListNotFound => "list-not-found",
            Self::DecryptFailed => "decrypt-failed",
            Self::NoAccount => "not-signed-in",
        }
    }

    pub(crate) const fn include_supported(&self) -> bool {
        matches!(self, Self::UnsupportedKind(_) | Self::UnsupportedType(_))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParsedOptions {
    pub(crate) create: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedItems {
    pub(crate) entries: Vec<ListEntry>,
    pub(crate) remove_private_matches: bool,
}

pub(crate) fn parse_selector(
    list: Option<&Value>,
    limits: ListsProviderLimits,
) -> Result<(&'static SupportedList, ListSelector), ListRefusal> {
    let list = list
        .and_then(Value::as_object)
        .ok_or(ListRefusal::MalformedSelector)?;
    if list
        .keys()
        .any(|key| !["kind", "type", "identifier"].contains(&key.as_str()))
    {
        return Err(ListRefusal::MalformedSelector);
    }
    let (kind, supported) = match (list.get("kind"), list.get("type")) {
        (Some(kind), None) => {
            let kind = kind
                .as_u64()
                .filter(|kind| *kind <= u64::from(u16::MAX))
                .ok_or(ListRefusal::MalformedSelector)? as u16;
            (
                kind,
                supported_list(kind).ok_or(ListRefusal::UnsupportedKind(kind))?,
            )
        }
        (None, Some(Value::String(list_type))) => {
            let supported = supported_list_type(list_type)
                .ok_or_else(|| ListRefusal::UnsupportedType(Arc::from(list_type.as_str())))?;
            (supported.kind, supported)
        }
        _ => return Err(ListRefusal::MalformedSelector),
    };
    let identifier = match list.get("identifier") {
        None | Some(Value::Null) => None,
        Some(Value::String(identifier)) => Some(identifier.as_str()),
        Some(_) => return Err(ListRefusal::MalformedSelector),
    };
    match (supported.addressable, identifier) {
        (true, None) => return Err(ListRefusal::IdentifierRequired(kind)),
        (false, Some(_)) => return Err(ListRefusal::IdentifierNotAllowed(kind)),
        _ => {}
    }
    if let Some(identifier) = identifier
        && (identifier.is_empty() || identifier.len() > limits.maximum_identifier_bytes)
    {
        return Err(ListRefusal::IdentifierBounds(
            limits.maximum_identifier_bytes,
        ));
    }
    Ok((
        supported,
        ListSelector {
            kind,
            identifier: identifier.map(Arc::from),
        },
    ))
}

pub(crate) fn parse_items(
    items: Option<&Value>,
    supported: &SupportedList,
    limits: ListsProviderLimits,
    action: ListsAction,
) -> Result<ParsedItems, ListRefusal> {
    let items = items
        .and_then(Value::as_array)
        .ok_or(ListRefusal::MalformedItems)?;
    if items.is_empty() || items.len() > limits.maximum_request_items {
        return Err(ListRefusal::ItemBounds(limits.maximum_request_items));
    }
    let mut parsed = Vec::with_capacity(items.len());
    let mut seen = BTreeSet::new();
    let mut remove_private_matches = false;
    for item in items {
        let (entry, visibility_omitted) = parse_item(item, supported, limits)?;
        if !seen.insert(entry.clone()) {
            return Err(ListRefusal::DuplicateItem);
        }
        parsed.push(entry);
        remove_private_matches |= action == ListsAction::Remove && visibility_omitted;
    }
    Ok(ParsedItems {
        entries: parsed,
        remove_private_matches,
    })
}

fn parse_item(
    item: &Value,
    supported: &SupportedList,
    limits: ListsProviderLimits,
) -> Result<(ListEntry, bool), ListRefusal> {
    let item = item.as_object().ok_or(ListRefusal::MalformedItems)?;
    if !item.contains_key("itemType")
        || !item.contains_key("value")
        || item.keys().any(|key| {
            !["itemType", "value", "relay", "label", "visibility"].contains(&key.as_str())
        })
    {
        return Err(ListRefusal::MalformedItems);
    }
    let item_type = item
        .get("itemType")
        .and_then(Value::as_str)
        .ok_or(ListRefusal::MalformedItems)?;
    let tag = parse_semantic_item_type(item_type)
        .ok_or_else(|| ListRefusal::UnsupportedItemType(Arc::from(item_type)))?;
    if !supported.accepts(tag) {
        return Err(ListRefusal::ItemTypeRejected {
            kind: supported.kind,
            tag,
        });
    }
    let value = item
        .get("value")
        .and_then(Value::as_str)
        .ok_or(ListRefusal::MalformedItems)?;
    validate_value(tag, value, limits)?;
    for hint in ["relay", "label"] {
        if let Some(value) = item.get(hint) {
            let value = value.as_str().ok_or(ListRefusal::MalformedItems)?;
            if value.is_empty() || value.len() > limits.maximum_value_bytes {
                return Err(ListRefusal::MalformedItems);
            }
            return Err(ListRefusal::ItemHintsUnsupported);
        }
    }
    let visibility_omitted = !item.contains_key("visibility");
    match item.get("visibility") {
        None => {}
        Some(Value::String(value)) if value == "public" => {}
        Some(Value::String(value)) if value == "private" => {
            return Err(ListRefusal::PrivateItemsUnsupported);
        }
        Some(_) => return Err(ListRefusal::MalformedItems),
    }
    Ok((ListEntry::new(tag, value), visibility_omitted))
}

pub(crate) fn parse_options(options: Option<&Value>) -> Result<ParsedOptions, ListRefusal> {
    let Some(options) = options else {
        return Ok(ParsedOptions::default());
    };
    let options = options.as_object().ok_or(ListRefusal::MalformedOptions)?;
    if options
        .keys()
        .any(|key| !["create", "title", "description", "image"].contains(&key.as_str()))
    {
        return Err(ListRefusal::MalformedOptions);
    }
    let create = match options.get("create") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err(ListRefusal::MalformedOptions),
    };
    for field in ["title", "description", "image"] {
        if let Some(value) = options.get(field) {
            let value = value.as_str().ok_or(ListRefusal::MalformedOptions)?;
            if value.is_empty() {
                return Err(ListRefusal::MalformedOptions);
            }
            return Err(ListRefusal::MetadataOptionsUnsupported);
        }
    }
    Ok(ParsedOptions { create })
}
