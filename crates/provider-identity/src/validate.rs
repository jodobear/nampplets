use nmp_native_runtime_core::BoundedJson;

use crate::{
    FrozenIdentity, IdentityProviderLimits, IdentityQuery, IdentityValue,
    types::MAX_SAFE_JSON_INTEGER,
};

pub(crate) fn wire_pubkey(identity: &FrozenIdentity) -> &str {
    identity
        .account
        .as_ref()
        .map_or("", |account| account.0.as_ref())
}

pub(crate) fn validate_identity(identity: &FrozenIdentity) -> Result<(), ()> {
    identity
        .account
        .as_ref()
        .map_or(Ok(()), |account| validate_pubkey(&account.0))
}

fn validate_pubkey(pubkey: &str) -> Result<(), ()> {
    if pubkey.len() == 64
        && pubkey
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(())
    }
}

fn validate_text(value: &str, limits: IdentityProviderLimits) -> Result<(), ()> {
    if value.len() <= limits.maximum_text_bytes {
        Ok(())
    } else {
        Err(())
    }
}

fn validate_string_list(values: &[String], limits: IdentityProviderLimits) -> Result<(), ()> {
    if values.len() > limits.maximum_items {
        return Err(());
    }
    values
        .iter()
        .try_for_each(|value| validate_text(value, limits))
}

fn validate_pubkey_list(values: &[String], limits: IdentityProviderLimits) -> Result<(), ()> {
    if values.len() > limits.maximum_items {
        return Err(());
    }
    values.iter().try_for_each(|value| validate_pubkey(value))
}

pub(crate) fn validate_value(
    query: &IdentityQuery,
    value: &IdentityValue,
    limits: IdentityProviderLimits,
) -> Result<(), ()> {
    match (query, value) {
        (IdentityQuery::Relays, IdentityValue::Relays(relays)) => {
            if relays.len() > limits.maximum_relays {
                return Err(());
            }
            relays
                .keys()
                .try_for_each(|relay| validate_text(relay, limits))
        }
        (IdentityQuery::Profile, IdentityValue::Profile(profile)) => {
            if let Some(profile) = profile {
                [
                    &profile.name,
                    &profile.display_name,
                    &profile.about,
                    &profile.picture,
                    &profile.banner,
                    &profile.nip05,
                    &profile.lud16,
                    &profile.website,
                ]
                .into_iter()
                .flatten()
                .try_for_each(|value| validate_text(value, limits))?;
            }
            Ok(())
        }
        (IdentityQuery::Follows, IdentityValue::Follows(values))
        | (IdentityQuery::Mutes, IdentityValue::Mutes(values))
        | (IdentityQuery::Blocked, IdentityValue::Blocked(values)) => {
            validate_pubkey_list(values, limits)
        }
        (IdentityQuery::List { .. }, IdentityValue::List(values)) => {
            validate_string_list(values, limits)
        }
        (IdentityQuery::Zaps, IdentityValue::Zaps(zaps)) => {
            if zaps.len() > limits.maximum_items {
                return Err(());
            }
            for zap in zaps {
                validate_text(&zap.event_id, limits)?;
                validate_pubkey(&zap.sender)?;
                if zap.amount > MAX_SAFE_JSON_INTEGER {
                    return Err(());
                }
                if let Some(content) = &zap.content {
                    validate_text(content, limits)?;
                }
            }
            Ok(())
        }
        (IdentityQuery::Badges, IdentityValue::Badges(badges)) => {
            if badges.len() > limits.maximum_items {
                return Err(());
            }
            for badge in badges {
                validate_text(&badge.id, limits)?;
                validate_pubkey(&badge.awarded_by)?;
                for value in [&badge.name, &badge.description, &badge.image]
                    .into_iter()
                    .flatten()
                {
                    validate_text(value, limits)?;
                }
                if let Some(thumbs) = &badge.thumbs {
                    if thumbs.len() > limits.maximum_thumbnails_per_badge {
                        return Err(());
                    }
                    thumbs
                        .iter()
                        .try_for_each(|thumb| validate_text(thumb, limits))?;
                }
            }
            Ok(())
        }
        _ => Err(()),
    }
}

pub(crate) fn validate_evidence(
    evidence: &BoundedJson,
    limits: IdentityProviderLimits,
) -> Result<(), ()> {
    if evidence.byte_len() > limits.maximum_evidence_bytes {
        return Err(());
    }
    let object = evidence.decode().map_err(|_| ())?;
    let object = object.as_object().ok_or(())?;
    if object.contains_key("synced") || object.contains_key("complete") {
        return Err(());
    }
    Ok(())
}
