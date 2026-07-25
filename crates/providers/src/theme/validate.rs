use nmp_native_runtime_core::BoundedJson;
use serde_json::Value;

use super::{ThemeError, ThemeProviderLimits};

pub(super) fn validate_limits(limits: ThemeProviderLimits) -> Result<(), ThemeError> {
    if [
        limits.maximum_theme_bytes,
        limits.maximum_response_bytes,
        limits.maximum_correlation_id_bytes,
        limits.maximum_string_bytes,
        limits.maximum_declaring_ready_sessions,
    ]
    .contains(&0)
        || limits.maximum_response_bytes < limits.maximum_theme_bytes
    {
        return Err(ThemeError::InvalidLimit);
    }
    Ok(())
}

pub(super) fn validate_theme(value: &Value, limits: ThemeProviderLimits) -> Result<(), ThemeError> {
    let object = value.as_object().ok_or(ThemeError::InvalidColors)?;
    let colors = object
        .get("colors")
        .and_then(Value::as_object)
        .ok_or(ThemeError::InvalidColors)?;
    for field in ["background", "text", "primary"] {
        if colors
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(|value| value.len() > limits.maximum_string_bytes)
        {
            return Err(ThemeError::InvalidColors);
        }
    }
    if colors.len() != 3 {
        return Err(ThemeError::InvalidColors);
    }
    if let Some(fonts) = object.get("fonts") {
        let fonts = fonts.as_object().ok_or(ThemeError::InvalidField("fonts"))?;
        if fonts
            .keys()
            .any(|field| !matches!(field.as_str(), "body" | "title"))
        {
            return Err(ThemeError::InvalidField("fonts"));
        }
        for font in fonts.values() {
            let font = font.as_object().ok_or(ThemeError::InvalidField("fonts"))?;
            if font.len() != 2
                || ["name", "url"].into_iter().any(|field| {
                    font.get(field)
                        .and_then(Value::as_str)
                        .is_none_or(|value| value.len() > limits.maximum_string_bytes)
                })
            {
                return Err(ThemeError::InvalidField("fonts"));
            }
        }
    }
    if let Some(background) = object.get("background") {
        let background = background
            .as_object()
            .ok_or(ThemeError::InvalidField("background"))?;
        if background.len() != 3
            || ["url", "mode", "mime"].into_iter().any(|field| {
                background
                    .get(field)
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.len() > limits.maximum_string_bytes)
            })
        {
            return Err(ThemeError::InvalidField("background"));
        }
    }
    if object.get("title").is_some_and(|title| {
        title
            .as_str()
            .is_none_or(|value| value.len() > limits.maximum_string_bytes)
    }) {
        return Err(ThemeError::InvalidField("title"));
    }
    if object
        .keys()
        .any(|field| !matches!(field.as_str(), "colors" | "fonts" | "background" | "title"))
    {
        return Err(ThemeError::InvalidField("unknown"));
    }
    BoundedJson::from_value(value, limits.maximum_theme_bytes)
        .map(|_| ())
        .map_err(|_| ThemeError::TooLarge {
            maximum_bytes: limits.maximum_theme_bytes,
        })
}
