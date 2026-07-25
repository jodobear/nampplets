use crate::{ResourceErrorCode, ResourceFailure, ResourceNetworkError, SvgRasterError};

pub(super) fn network_failure(error: ResourceNetworkError) -> ResourceFailure {
    match error {
        ResourceNetworkError::Cancelled => cancelled(),
        ResourceNetworkError::Timeout => failure(
            ResourceErrorCode::Timeout,
            "resource network operation timed out",
        ),
        ResourceNetworkError::NotFound => {
            failure(ResourceErrorCode::NotFound, "resource was not found")
        }
        ResourceNetworkError::Failed => failure(
            ResourceErrorCode::NetworkError,
            "resource network operation failed",
        ),
        ResourceNetworkError::TooLarge => too_large(),
    }
}

pub(super) fn svg_failure(error: SvgRasterError) -> ResourceFailure {
    match error {
        SvgRasterError::Cancelled => cancelled(),
        SvgRasterError::Timeout => {
            failure(ResourceErrorCode::Timeout, "SVG rasterization timed out")
        }
        SvgRasterError::DecodeFailed => {
            failure(ResourceErrorCode::DecodeFailed, "SVG rasterization failed")
        }
        SvgRasterError::TooLarge => too_large(),
    }
}

pub(super) fn too_large() -> ResourceFailure {
    failure(
        ResourceErrorCode::TooLarge,
        "resource bytes exceeded the configured limit",
    )
}

pub(super) fn cancelled() -> ResourceFailure {
    failure(
        ResourceErrorCode::NetworkError,
        "resource acquisition was cancelled",
    )
}

pub(super) fn failure(code: ResourceErrorCode, message: &'static str) -> ResourceFailure {
    ResourceFailure::new(code, message)
}
