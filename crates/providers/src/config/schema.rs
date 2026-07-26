use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::MAX_SCHEMA_DEPTH;
use super::types::{ConfigError, ConfigProviderLimits, ConfigSchemaErrorCode};
use super::value::validate_value;
use super::wire::{bounded, schema_error};

pub(super) fn validate_schema(
    schema: &Value,
    limits: ConfigProviderLimits,
) -> Result<(), ConfigError> {
    bounded(schema, limits.maximum_schema_bytes)?;
    let root = schema.as_object().ok_or_else(|| {
        schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "schema root must be an object",
        )
    })?;
    if root.get("type").and_then(Value::as_str) != Some("object")
        || !root.get("properties").is_some_and(Value::is_object)
    {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "schema root must declare object type and properties",
        ));
    }
    validate_schema_node(root, 1, true, limits)
}

fn validate_schema_node(
    schema: &Map<String, Value>,
    object_depth: usize,
    root: bool,
    limits: ConfigProviderLimits,
) -> Result<(), ConfigError> {
    for keyword in schema.keys() {
        if matches!(keyword.as_str(), "$ref" | "definitions" | "$defs") {
            return Err(schema_error(
                ConfigSchemaErrorCode::RefNotAllowed,
                format!("`{keyword}` is not allowed"),
            ));
        }
        if keyword == "pattern" {
            return Err(schema_error(
                ConfigSchemaErrorCode::PatternNotAllowed,
                "`pattern` is not allowed",
            ));
        }
        if matches!(
            keyword.as_str(),
            "oneOf"
                | "anyOf"
                | "allOf"
                | "not"
                | "if"
                | "then"
                | "else"
                | "patternProperties"
                | "propertyNames"
                | "dependencies"
                | "dependentSchemas"
                | "unevaluatedProperties"
                | "$dynamicRef"
        ) {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{keyword}` is outside the Core Subset"),
            ));
        }
        if !is_allowed_keyword(keyword, root) && !keyword.starts_with("x-napplet-") {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{keyword}` is outside the Core Subset"),
            ));
        }
    }

    if let Some(draft) = schema.get("$schema") {
        let Some(draft) = draft.as_str() else {
            return Err(schema_error(
                ConfigSchemaErrorCode::UnsupportedDraft,
                "`$schema` must be a supported draft URI",
            ));
        };
        if !matches!(
            draft,
            "http://json-schema.org/draft-07/schema#"
                | "https://json-schema.org/draft-07/schema"
                | "https://json-schema.org/draft/2019-09/schema"
                | "https://json-schema.org/draft/2020-12/schema"
        ) {
            return Err(schema_error(
                ConfigSchemaErrorCode::UnsupportedDraft,
                "only JSON Schema draft-07, 2019-09, and 2020-12 are supported",
            ));
        }
    }

    let schema_type = schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| schema_error(ConfigSchemaErrorCode::InvalidSchema, "`type` is required"))?;
    if !matches!(
        schema_type,
        "object" | "string" | "number" | "integer" | "boolean" | "array"
    ) {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "schema type is outside the Core Subset",
        ));
    }
    if root && schema_type != "object" {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "schema root type must be object",
        ));
    }

    for annotation in [
        "title",
        "description",
        "deprecationMessage",
        "markdownDescription",
        "format",
        "x-napplet-section",
    ] {
        if schema
            .get(annotation)
            .is_some_and(|value| !value.is_string())
        {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{annotation}` must be a string"),
            ));
        }
    }
    if schema
        .get("x-napplet-secret")
        .is_some_and(|value| !value.is_boolean())
        || schema.get("x-napplet-order").is_some_and(|value| {
            value
                .as_f64()
                .is_none_or(|number| !number.is_finite() || number < 0.0)
        })
    {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "invalid x-napplet extension value",
        ));
    }
    if schema.get("x-napplet-secret") == Some(&Value::Bool(true)) && schema.contains_key("default")
    {
        return Err(schema_error(
            ConfigSchemaErrorCode::SecretWithDefault,
            "secret properties cannot declare a default",
        ));
    }

    validate_numeric_constraints(schema)?;
    if let Some(enum_values) = schema.get("enum") {
        let Some(enum_values) = enum_values.as_array() else {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                "`enum` must be an array",
            ));
        };
        if enum_values.is_empty() || enum_values.len() > limits.maximum_enum_items {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                "`enum` has an invalid item count",
            ));
        }
        if let Some(descriptions) = schema.get("enumDescriptions") {
            let descriptions = descriptions.as_array().ok_or_else(|| {
                schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "`enumDescriptions` must be an array",
                )
            })?;
            if descriptions.len() != enum_values.len()
                || descriptions.iter().any(|value| !value.is_string())
            {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "`enumDescriptions` must parallel `enum` with strings",
                ));
            }
        }
    } else if schema.contains_key("enumDescriptions") {
        return Err(schema_error(
            ConfigSchemaErrorCode::InvalidSchema,
            "`enumDescriptions` requires `enum`",
        ));
    }

    match schema_type {
        "object" => {
            if object_depth > MAX_SCHEMA_DEPTH {
                return Err(schema_error(
                    ConfigSchemaErrorCode::SchemaTooDeep,
                    "object nesting exceeds 4 levels",
                ));
            }
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    schema_error(
                        ConfigSchemaErrorCode::InvalidSchema,
                        "object schemas must declare `properties`",
                    )
                })?;
            if properties.len() > limits.maximum_properties_per_object {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "object property count exceeds the configured limit",
                ));
            }
            if schema
                .get("additionalProperties")
                .is_some_and(|value| !value.is_boolean())
            {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "`additionalProperties` must be boolean",
                ));
            }
            if let Some(required) = schema.get("required") {
                let required = required.as_array().ok_or_else(|| {
                    schema_error(
                        ConfigSchemaErrorCode::InvalidSchema,
                        "`required` must be an array",
                    )
                })?;
                let mut unique = BTreeSet::new();
                for field in required {
                    let Some(field) = field.as_str() else {
                        return Err(schema_error(
                            ConfigSchemaErrorCode::InvalidSchema,
                            "`required` entries must be strings",
                        ));
                    };
                    if !properties.contains_key(field) || !unique.insert(field) {
                        return Err(schema_error(
                            ConfigSchemaErrorCode::InvalidSchema,
                            "`required` entries must be unique declared properties",
                        ));
                    }
                }
            }
            for property in properties.values() {
                let property = property.as_object().ok_or_else(|| {
                    schema_error(
                        ConfigSchemaErrorCode::InvalidSchema,
                        "property schemas must be objects",
                    )
                })?;
                let next_depth = if property.get("type").and_then(Value::as_str) == Some("object") {
                    object_depth.saturating_add(1)
                } else {
                    object_depth
                };
                validate_schema_node(property, next_depth, false, limits)?;
            }
        }
        "array" => {
            let items = schema
                .get("items")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    schema_error(
                        ConfigSchemaErrorCode::InvalidSchema,
                        "array `items` must be one homogeneous primitive schema",
                    )
                })?;
            if !matches!(
                items.get("type").and_then(Value::as_str),
                Some("string" | "number" | "integer" | "boolean")
            ) {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "Core Subset arrays may contain primitives only",
                ));
            }
            validate_schema_node(items, object_depth, false, limits)?;
        }
        _ => {
            if schema.contains_key("properties")
                || schema.contains_key("required")
                || schema.contains_key("items")
                || schema.contains_key("additionalProperties")
            {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "container keywords do not match the declared type",
                ));
            }
        }
    }
    if let Some(default) = schema.get("default") {
        validate_value(
            &Value::Object(schema.clone()),
            default,
            object_depth,
            limits,
        )
        .map_err(|reason| schema_error(ConfigSchemaErrorCode::InvalidSchema, reason))?;
    }
    Ok(())
}

fn is_allowed_keyword(keyword: &str, root: bool) -> bool {
    matches!(
        keyword,
        "type"
            | "properties"
            | "required"
            | "items"
            | "additionalProperties"
            | "default"
            | "title"
            | "description"
            | "enum"
            | "enumDescriptions"
            | "minimum"
            | "maximum"
            | "minLength"
            | "maxLength"
            | "minItems"
            | "maxItems"
            | "format"
            | "deprecationMessage"
            | "markdownDescription"
            | "$schema"
            | "$id"
            | "id"
    ) || (root && keyword == "$version")
}

fn validate_numeric_constraints(schema: &Map<String, Value>) -> Result<(), ConfigError> {
    for keyword in ["minimum", "maximum"] {
        if schema
            .get(keyword)
            .is_some_and(|value| value.as_f64().is_none_or(|number| !number.is_finite()))
        {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{keyword}` must be a finite number"),
            ));
        }
    }
    for keyword in ["minLength", "maxLength", "minItems", "maxItems", "$version"] {
        if schema
            .get(keyword)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(schema_error(
                ConfigSchemaErrorCode::InvalidSchema,
                format!("`{keyword}` must be an unsigned integer"),
            ));
        }
    }
    for (minimum, maximum) in [
        ("minimum", "maximum"),
        ("minLength", "maxLength"),
        ("minItems", "maxItems"),
    ] {
        if let (Some(minimum), Some(maximum)) = (schema.get(minimum), schema.get(maximum)) {
            let valid = match (minimum.as_f64(), maximum.as_f64()) {
                (Some(minimum), Some(maximum)) => minimum <= maximum,
                _ => false,
            };
            if !valid {
                return Err(schema_error(
                    ConfigSchemaErrorCode::InvalidSchema,
                    "minimum constraint exceeds maximum constraint",
                ));
            }
        }
    }
    Ok(())
}
