//! Explicit, bounded environment identity for comparable evidence.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeasurementAvailability {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub environment_class: String,
    pub os: String,
    pub hardware: String,
    pub power_state: String,
    pub thermal_state: String,
    pub measurement_availability: AvailabilitySet,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailabilitySet {
    pub cpu_time_ns: MeasurementAvailability,
    pub peak_rss_bytes: MeasurementAvailability,
}

impl Environment {
    pub fn bounded(
        environment_class: impl Into<String>,
        os: impl Into<String>,
        hardware: impl Into<String>,
        power_state: impl Into<String>,
        thermal_state: impl Into<String>,
        availability: MeasurementAvailability,
    ) -> Result<Self, EnvironmentError> {
        let value = Self {
            environment_class: environment_class.into(),
            os: os.into(),
            hardware: hardware.into(),
            power_state: power_state.into(),
            thermal_state: thermal_state.into(),
            measurement_availability: AvailabilitySet {
                cpu_time_ns: availability,
                peak_rss_bytes: availability,
            },
        };
        validate_id(&value.environment_class)?;
        validate_text(&value.os, 256)?;
        validate_text(&value.hardware, 256)?;
        validate_text(&value.power_state, 64)?;
        validate_text(&value.thermal_state, 64)?;
        Ok(value)
    }
}

fn validate_id(value: &str) -> Result<(), EnvironmentError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && b"._:-".contains(&byte))
        })
    {
        return Err(EnvironmentError::InvalidEnvironmentClass);
    }
    Ok(())
}

fn validate_text(value: &str, maximum: usize) -> Result<(), EnvironmentError> {
    if value.is_empty() || value.len() > maximum {
        Err(EnvironmentError::InvalidBoundedText)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum EnvironmentError {
    #[error("environment class is not a bounded v1 identifier")]
    InvalidEnvironmentClass,
    #[error("environment fact is empty or exceeds its v1 bound")]
    InvalidBoundedText,
}
