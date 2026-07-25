use nmp_native_runtime_core::{CapabilityRequest, GrantDecision, Principal, WriteReceiptId};

use crate::{ActivityRecord, StoreError, StoreLimits};

pub(crate) fn validate_limits(limits: StoreLimits) -> Result<(), StoreError> {
    if [
        limits.maximum_installs,
        limits.maximum_install_title_bytes,
        limits.maximum_install_search_query_bytes,
        limits.maximum_grants_per_principal,
        limits.maximum_kv_keys_per_scope,
        limits.maximum_kv_bytes_per_scope,
        limits.maximum_value_bytes,
        limits.maximum_workspaces,
        limits.maximum_workspace_bytes,
        limits.maximum_workspace_assignments,
        limits.maximum_retained_receipts_per_workspace,
        limits.maximum_retained_receipt_bytes_per_workspace,
        limits.maximum_activity_facts,
        limits.maximum_activity_string_bytes,
        limits.maximum_activity_record_bytes,
        limits.maximum_activity_total_bytes,
    ]
    .contains(&0)
        || limits.maximum_activity_record_bytes > limits.maximum_activity_total_bytes
    {
        return Err(StoreError::InvalidLimits);
    }
    Ok(())
}

pub(crate) fn validate_install_title(title: &str, maximum: usize) -> Result<(), StoreError> {
    if title.is_empty()
        || title
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(StoreError::InvalidInstallTitle);
    }
    if title.len() > maximum {
        return Err(StoreError::InstallTitleTooLarge {
            actual: title.len(),
            maximum,
        });
    }
    Ok(())
}

pub(crate) fn validate_install_search_query(query: &str, maximum: usize) -> Result<(), StoreError> {
    if query
        .bytes()
        .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(StoreError::InvalidInstallSearchQuery);
    }
    if query.len() > maximum {
        return Err(StoreError::InstallSearchQueryTooLarge {
            actual: query.len(),
            maximum,
        });
    }
    Ok(())
}

pub(crate) fn encode_capability_requests(
    requests: &[CapabilityRequest],
    maximum: usize,
) -> Result<String, StoreError> {
    validate_capability_requests(requests, maximum)?;
    serde_json::to_string(requests).map_err(|error| StoreError::Corrupt(error.to_string()))
}

pub(crate) fn decode_capability_requests(
    encoded: &str,
    maximum: usize,
) -> Result<Vec<CapabilityRequest>, StoreError> {
    let requests: Vec<CapabilityRequest> =
        serde_json::from_str(encoded).map_err(|error| StoreError::Corrupt(error.to_string()))?;
    validate_capability_requests(&requests, maximum)?;
    Ok(requests)
}

fn validate_capability_requests(
    requests: &[CapabilityRequest],
    maximum: usize,
) -> Result<(), StoreError> {
    if requests.len() > maximum {
        return Err(StoreError::CapabilityRequestCapacity {
            actual: requests.len(),
            maximum,
        });
    }
    let unique = requests
        .iter()
        .map(|request| &request.capability)
        .collect::<std::collections::BTreeSet<_>>();
    if unique.len() != requests.len() {
        return Err(StoreError::DuplicateCapabilityRequest);
    }
    Ok(())
}

pub(crate) fn contains_search(value: &str, query: &str) -> bool {
    query.is_empty()
        || value
            .as_bytes()
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query.as_bytes()))
}

pub(crate) fn encode_retained_receipts(
    receipts: &[WriteReceiptId],
    limits: StoreLimits,
) -> Result<String, StoreError> {
    if receipts.len() > limits.maximum_retained_receipts_per_workspace {
        return Err(StoreError::RetainedReceiptCapacity {
            actual: receipts.len(),
            maximum: limits.maximum_retained_receipts_per_workspace,
        });
    }
    let encoded =
        serde_json::to_string(receipts).map_err(|error| StoreError::Corrupt(error.to_string()))?;
    validate_retained_receipts(receipts, encoded.len(), limits)?;
    Ok(encoded)
}

pub(crate) fn validate_retained_receipts(
    receipts: &[WriteReceiptId],
    encoded_bytes: usize,
    limits: StoreLimits,
) -> Result<(), StoreError> {
    if receipts.len() > limits.maximum_retained_receipts_per_workspace {
        return Err(StoreError::RetainedReceiptCapacity {
            actual: receipts.len(),
            maximum: limits.maximum_retained_receipts_per_workspace,
        });
    }
    if encoded_bytes > limits.maximum_retained_receipt_bytes_per_workspace {
        return Err(StoreError::RetainedReceiptBytes {
            actual: encoded_bytes,
            maximum: limits.maximum_retained_receipt_bytes_per_workspace,
        });
    }
    Ok(())
}

pub(crate) fn validate_activity(
    record: &ActivityRecord,
    limits: StoreLimits,
) -> Result<usize, StoreError> {
    let fields = [
        ("category", record.category.as_ref()),
        ("operation", record.operation.as_ref()),
        ("outcome", record.outcome.as_ref()),
    ];
    for (field, value) in fields {
        if value.is_empty()
            || value
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(StoreError::InvalidActivityString { field });
        }
        if value.len() > limits.maximum_activity_string_bytes {
            return Err(StoreError::ActivityStringTooLarge {
                field,
                actual: value.len(),
                maximum: limits.maximum_activity_string_bytes,
            });
        }
    }
    let bytes = fields.iter().fold(0usize, |total, (_, value)| {
        total.saturating_add(value.len())
    });
    let maximum = limits
        .maximum_activity_record_bytes
        .min(limits.maximum_activity_total_bytes);
    if bytes > maximum {
        return Err(StoreError::ActivityRecordTooLarge {
            actual: bytes,
            maximum,
        });
    }
    Ok(bytes)
}

pub(crate) fn validate_scope_name(field: &'static str, value: &str) -> Result<(), StoreError> {
    if value.is_empty()
        || value.len() > 256
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(StoreError::InvalidName { field });
    }
    Ok(())
}

pub(crate) fn principal_params(principal: &Principal) -> [&str; 3] {
    [
        principal.manifest_author(),
        principal.d_tag(),
        principal.aggregate_hash(),
    ]
}

pub(crate) fn grant_decision_text(decision: GrantDecision) -> &'static str {
    match decision {
        GrantDecision::Denied => "denied",
        GrantDecision::AskEveryTime => "ask_every_time",
        GrantDecision::AllowSession => "allow_session",
        GrantDecision::AllowExactBuild => "allow_exact_build",
        GrantDecision::Managed => "managed",
    }
}

pub(crate) fn parse_grant_decision(value: &str) -> Result<GrantDecision, StoreError> {
    match value {
        "denied" => Ok(GrantDecision::Denied),
        "ask_every_time" => Ok(GrantDecision::AskEveryTime),
        "allow_session" => Ok(GrantDecision::AllowSession),
        "allow_exact_build" => Ok(GrantDecision::AllowExactBuild),
        "managed" => Ok(GrantDecision::Managed),
        other => Err(StoreError::Corrupt(format!(
            "unknown grant decision {other}"
        ))),
    }
}
