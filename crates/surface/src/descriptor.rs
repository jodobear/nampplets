use std::{collections::BTreeSet, fmt};

use scraper::{Html, Selector};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorLimits {
    pub maximum_bytes: usize,
    pub maximum_depth: usize,
    pub maximum_inputs: usize,
    pub maximum_actions: usize,
}

impl Default for DescriptorLimits {
    fn default() -> Self {
        Self {
            maximum_bytes: 64 * 1024,
            maximum_depth: 16,
            maximum_inputs: 32,
            maximum_actions: 64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceDescriptor {
    pub schema: String,
    pub profile: SurfaceProfile,
    pub archetype: String,
    #[serde(default)]
    pub inputs: Vec<InputDescriptor>,
    #[serde(default)]
    pub actions: Vec<ActionDescriptor>,
    pub fallback: Fallback,
    #[serde(default)]
    pub presentation: Option<Presentation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceProfile {
    Renderer,
    Hybrid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDescriptor {
    pub name: String,
    pub schema: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDescriptor {
    pub name: String,
    pub schema: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    Legacy,
    Unavailable,
    Reject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presentation {
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedSurface {
    Legacy,
    Surface(SurfaceDescriptor),
}

pub fn parse_descriptor(
    verified_index_html: &[u8],
    limits: DescriptorLimits,
) -> Result<ParsedSurface, DescriptorError> {
    if limits.maximum_bytes == 0
        || limits.maximum_depth == 0
        || limits.maximum_inputs == 0
        || limits.maximum_actions == 0
    {
        return Err(DescriptorError::InvalidLimits);
    }
    let html =
        std::str::from_utf8(verified_index_html).map_err(|_| DescriptorError::IndexIsNotUtf8)?;
    let document = Html::parse_document(html);
    let selector = Selector::parse("script#napplet-surface").expect("static selector is valid");
    let matching = document.select(&selector).collect::<Vec<_>>();
    if matching.is_empty() {
        return Ok(ParsedSurface::Legacy);
    }
    if matching.len() != 1 {
        return Err(DescriptorError::MultipleDescriptors);
    }
    let element = matching[0];
    if element.value().attr("type") != Some("application/napplet-surface+json") {
        return Err(DescriptorError::InvalidScriptType);
    }
    let descriptor_json = element.text().collect::<String>();
    if descriptor_json.len() > limits.maximum_bytes {
        return Err(DescriptorError::TooLarge {
            actual: descriptor_json.len(),
            maximum: limits.maximum_bytes,
        });
    }
    let mut deserializer = serde_json::Deserializer::from_str(&descriptor_json);
    let value = UniqueValueSeed
        .deserialize(&mut deserializer)
        .map_err(|error| DescriptorError::InvalidJson(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| DescriptorError::InvalidJson(error.to_string()))?;
    if value_depth(&value) > limits.maximum_depth {
        return Err(DescriptorError::TooDeep {
            maximum: limits.maximum_depth,
        });
    }
    let descriptor: SurfaceDescriptor = serde_json::from_value(value)
        .map_err(|error| DescriptorError::InvalidShape(error.to_string()))?;
    validate_descriptor(&descriptor, limits)?;
    Ok(ParsedSurface::Surface(descriptor))
}

fn validate_descriptor(
    descriptor: &SurfaceDescriptor,
    limits: DescriptorLimits,
) -> Result<(), DescriptorError> {
    if descriptor.schema != "nmp.surface/1" {
        return Err(DescriptorError::UnsupportedSchema(
            descriptor.schema.clone(),
        ));
    }
    validate_registry_name("archetype", &descriptor.archetype)?;
    if descriptor.inputs.len() > limits.maximum_inputs {
        return Err(DescriptorError::TooManyInputs {
            actual: descriptor.inputs.len(),
            maximum: limits.maximum_inputs,
        });
    }
    if descriptor.actions.len() > limits.maximum_actions {
        return Err(DescriptorError::TooManyActions {
            actual: descriptor.actions.len(),
            maximum: limits.maximum_actions,
        });
    }
    let mut input_names = BTreeSet::new();
    for input in &descriptor.inputs {
        validate_registry_name("input name", &input.name)?;
        validate_schema(&input.schema)?;
        if !input_names.insert(input.name.as_str()) {
            return Err(DescriptorError::DuplicateInput(input.name.clone()));
        }
    }
    let mut action_names = BTreeSet::new();
    for action in &descriptor.actions {
        validate_registry_name("action name", &action.name)?;
        validate_schema(&action.schema)?;
        if !action_names.insert(action.name.as_str()) {
            return Err(DescriptorError::DuplicateAction(action.name.clone()));
        }
    }
    if let Some(presentation) = &descriptor.presentation {
        validate_registry_name("presentation kind", &presentation.kind)?;
    }
    Ok(())
}

fn validate_registry_name(field: &'static str, value: &str) -> Result<(), DescriptorError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
        })
    {
        return Err(DescriptorError::InvalidIdentifier {
            field,
            value: value.to_owned(),
        });
    }
    Ok(())
}

fn validate_schema(value: &str) -> Result<(), DescriptorError> {
    if value.is_empty()
        || value.len() > 192
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'-' | b'_' | b'/')
        })
    {
        return Err(DescriptorError::InvalidIdentifier {
            field: "schema",
            value: value.to_owned(),
        });
    }
    let Some((name, version)) = value.rsplit_once('/') else {
        return Err(DescriptorError::UnversionedSchema(value.to_owned()));
    };
    if name.is_empty()
        || name.contains('/')
        || version.is_empty()
        || !version.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(DescriptorError::UnversionedSchema(value.to_owned()));
    }
    Ok(())
}

fn value_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(value_depth).max().unwrap_or_default(),
        Value::Object(values) => 1 + values.values().map(value_depth).max().unwrap_or_default(),
        _ => 1,
    }
}

struct UniqueValueSeed;

impl<'de> DeserializeSeed<'de> for UniqueValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor)
    }
}

struct UniqueValueVisitor;

impl<'de> Visitor<'de> for UniqueValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        UniqueValueSeed.deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(UniqueValueSeed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom(format!("duplicate key {key}")));
            }
            let value = map.next_value_seed(UniqueValueSeed)?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DescriptorError {
    #[error("descriptor limits must be finite and non-zero")]
    InvalidLimits,
    #[error("verified index is not UTF-8")]
    IndexIsNotUtf8,
    #[error("more than one napplet surface descriptor exists")]
    MultipleDescriptors,
    #[error("napplet-surface script has the wrong inert MIME type")]
    InvalidScriptType,
    #[error("surface descriptor is {actual} bytes; the maximum is {maximum}")]
    TooLarge { actual: usize, maximum: usize },
    #[error("surface descriptor exceeds maximum JSON depth {maximum}")]
    TooDeep { maximum: usize },
    #[error("invalid surface descriptor JSON: {0}")]
    InvalidJson(String),
    #[error("invalid surface descriptor shape: {0}")]
    InvalidShape(String),
    #[error("unsupported surface schema {0}")]
    UnsupportedSchema(String),
    #[error("invalid {field} identifier {value}")]
    InvalidIdentifier { field: &'static str, value: String },
    #[error("schema identifier must end in a numeric local version: {0}")]
    UnversionedSchema(String),
    #[error("surface declares {actual} inputs; maximum is {maximum}")]
    TooManyInputs { actual: usize, maximum: usize },
    #[error("surface declares {actual} actions; maximum is {maximum}")]
    TooManyActions { actual: usize, maximum: usize },
    #[error("duplicate input {0}")]
    DuplicateInput(String),
    #[error("duplicate action {0}")]
    DuplicateAction(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(json: &str) -> Vec<u8> {
        format!(
            r#"<html><script id="napplet-surface" type="application/napplet-surface+json">{json}</script></html>"#
        )
        .into_bytes()
    }

    #[test]
    fn descriptor_less_is_legacy() {
        assert_eq!(
            parse_descriptor(b"<html></html>", DescriptorLimits::default()).unwrap(),
            ParsedSurface::Legacy
        );
    }

    #[test]
    fn duplicate_json_key_is_rejected() {
        let source = html(
            r#"{"schema":"nmp.surface/1","schema":"nmp.surface/1","profile":"renderer","archetype":"feed","fallback":"legacy"}"#,
        );
        assert!(matches!(
            parse_descriptor(&source, DescriptorLimits::default()),
            Err(DescriptorError::InvalidJson(message)) if message.contains("duplicate key")
        ));
    }

    #[test]
    fn valid_descriptor_is_detected_before_execution() {
        let source = html(
            r#"{"schema":"nmp.surface/1","profile":"renderer","archetype":"feed","inputs":[{"name":"items","schema":"nostr.events.collection/1","required":true}],"actions":[{"name":"profile.open","schema":"nostr.pubkey-ref/1"}],"fallback":"legacy"}"#,
        );
        let ParsedSurface::Surface(descriptor) =
            parse_descriptor(&source, DescriptorLimits::default()).unwrap()
        else {
            panic!("expected surface");
        };
        assert_eq!(descriptor.profile, SurfaceProfile::Renderer);
        assert_eq!(descriptor.inputs[0].name, "items");
    }
}
