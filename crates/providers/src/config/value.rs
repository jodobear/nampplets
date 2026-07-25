use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use nmp_native_nap_bridge::ProviderPushSender;
use nmp_native_runtime_core::{Principal, SessionId};
use serde_json::{Map, Value};

use super::ConfigSession;
use super::MAX_SCHEMA_DEPTH;
use super::types::{ConfigError, ConfigProviderLimits};
use super::wire::bounded;

pub(super) fn validate_value(
    schema: &Value,
    value: &Value,
    depth: usize,
    limits: ConfigProviderLimits,
) -> Result<(), Arc<str>> {
    if depth > MAX_SCHEMA_DEPTH.saturating_add(1) {
        return Err(Arc::from("value nesting exceeds the schema depth limit"));
    }
    if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > limits.maximum_values_bytes) {
        return Err(Arc::from("value exceeds the configured byte limit"));
    }
    let schema = schema
        .as_object()
        .ok_or_else(|| Arc::from("schema node is not an object"))?;
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        if !enum_values.contains(value) {
            return Err(Arc::from("value is outside the declared enum"));
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => {
            let value = value
                .as_str()
                .ok_or_else(|| Arc::from("value must be a string"))?;
            if value.len() > limits.maximum_string_bytes {
                return Err(Arc::from("string exceeds the configured byte limit"));
            }
            validate_length(schema, value.chars().count(), "minLength", "maxLength")
        }
        Some("number") => {
            let number = value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| Arc::from("value must be a finite number"))?;
            validate_number(schema, number)
        }
        Some("integer") => {
            let number = value
                .as_i64()
                .map(|number| number as f64)
                .or_else(|| value.as_u64().map(|number| number as f64))
                .ok_or_else(|| Arc::from("value must be an integer"))?;
            validate_number(schema, number)
        }
        Some("boolean") => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(Arc::from("value must be a boolean"))
            }
        }
        Some("array") => {
            let values = value
                .as_array()
                .ok_or_else(|| Arc::from("value must be an array"))?;
            if values.len() > limits.maximum_array_items {
                return Err(Arc::from("array exceeds the configured item limit"));
            }
            validate_length(schema, values.len(), "minItems", "maxItems")?;
            let items = schema
                .get("items")
                .ok_or_else(|| Arc::from("array schema is missing items"))?;
            for value in values {
                validate_value(items, value, depth.saturating_add(1), limits)?;
            }
            Ok(())
        }
        Some("object") => {
            let object = value
                .as_object()
                .ok_or_else(|| Arc::from("value must be an object"))?;
            if object.len() > limits.maximum_properties_per_object {
                return Err(Arc::from("object exceeds the configured property limit"));
            }
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| Arc::from("object schema is missing properties"))?;
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for required in required {
                    if required
                        .as_str()
                        .is_some_and(|field| !object.contains_key(field))
                    {
                        return Err(Arc::from("required property is missing"));
                    }
                }
            }
            let allow_additional = schema
                .get("additionalProperties")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            for (field, value) in object {
                if let Some(property_schema) = properties.get(field) {
                    validate_value(property_schema, value, depth.saturating_add(1), limits)?;
                } else if !allow_additional {
                    return Err(Arc::from("object contains an undeclared property"));
                }
            }
            Ok(())
        }
        _ => Err(Arc::from("schema type is unsupported")),
    }
}

fn validate_length(
    schema: &Map<String, Value>,
    actual: usize,
    minimum: &str,
    maximum: &str,
) -> Result<(), Arc<str>> {
    if schema
        .get(minimum)
        .and_then(Value::as_u64)
        .is_some_and(|minimum| actual < usize::try_from(minimum).unwrap_or(usize::MAX))
    {
        return Err(Arc::from("value is shorter than the minimum"));
    }
    if schema
        .get(maximum)
        .and_then(Value::as_u64)
        .is_some_and(|maximum| actual > usize::try_from(maximum).unwrap_or(usize::MAX))
    {
        return Err(Arc::from("value exceeds the maximum length"));
    }
    Ok(())
}

fn validate_number(schema: &Map<String, Value>, number: f64) -> Result<(), Arc<str>> {
    if schema
        .get("minimum")
        .and_then(Value::as_f64)
        .is_some_and(|minimum| number < minimum)
    {
        return Err(Arc::from("number is below the minimum"));
    }
    if schema
        .get("maximum")
        .and_then(Value::as_f64)
        .is_some_and(|maximum| number > maximum)
    {
        return Err(Arc::from("number exceeds the maximum"));
    }
    Ok(())
}

pub(super) fn resolve_root(
    schema: &Value,
    persisted: Option<&Map<String, Value>>,
    limits: ConfigProviderLimits,
) -> Result<Value, ConfigError> {
    let resolved = resolve_object(
        schema,
        persisted,
        schema.get("default").and_then(Value::as_object),
        0,
        limits,
    )?;
    bounded(&resolved, limits.maximum_values_bytes)?;
    Ok(resolved)
}

fn resolve_object(
    schema: &Value,
    persisted: Option<&Map<String, Value>>,
    ancestor_default: Option<&Map<String, Value>>,
    depth: usize,
    limits: ConfigProviderLimits,
) -> Result<Value, ConfigError> {
    let schema = schema
        .as_object()
        .ok_or_else(|| ConfigError::InvalidValues(Arc::from("schema node is invalid")))?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| ConfigError::InvalidValues(Arc::from("object schema has no properties")))?;
    let mut resolved = Map::new();
    for (field, property_schema) in properties {
        let candidate = persisted
            .and_then(|object| object.get(field))
            .filter(|value| validate_value(property_schema, value, depth, limits).is_ok())
            .or_else(|| property_schema.get("default"))
            .or_else(|| ancestor_default.and_then(|object| object.get(field)));
        let Some(candidate) = candidate else {
            continue;
        };
        let value = if property_schema.get("type").and_then(Value::as_str) == Some("object") {
            let Some(candidate) = candidate.as_object() else {
                continue;
            };
            resolve_object(
                property_schema,
                Some(candidate),
                property_schema.get("default").and_then(Value::as_object),
                depth.saturating_add(1),
                limits,
            )?
        } else {
            if validate_value(property_schema, candidate, depth, limits).is_err() {
                continue;
            }
            candidate.clone()
        };
        resolved.insert(field.clone(), value);
    }
    let value = Value::Object(resolved);
    validate_value(&Value::Object(schema.clone()), &value, depth, limits)
        .map_err(ConfigError::InvalidValues)?;
    Ok(value)
}

pub(super) fn matching_targets(
    sessions: &BTreeMap<SessionId, ConfigSession>,
    principal: &Principal,
) -> Vec<ProviderPushSender> {
    sessions
        .values()
        .filter(|session| session.subscribed && &session.principal == principal)
        .map(|session| session.outbound.clone())
        .collect()
}

pub(super) fn schema_sections(schema: &Value) -> BTreeSet<&str> {
    fn visit<'a>(schema: &'a Value, sections: &mut BTreeSet<&'a str>) {
        let Some(schema) = schema.as_object() else {
            return;
        };
        if let Some(section) = schema.get("x-napplet-section").and_then(Value::as_str) {
            sections.insert(section);
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for property in properties.values() {
                visit(property, sections);
            }
        }
    }
    let mut sections = BTreeSet::new();
    visit(schema, &mut sections);
    sections
}
