//! URL/MIME/address acquisition policy, split by concern: HTTPS and data/
//! blossom URL handling in [`url`], MIME sniffing in [`mime`], public-IP
//! policy in [`address`], and [`ResourceFailure`] construction in
//! [`failure`]. [`AcquisitionContext`] ties them together into the single
//! bounded `acquire` entry point.

mod address;
mod failure;
mod mime;
#[cfg(test)]
mod tests;
mod url;

use std::{net::IpAddr, sync::Arc};

use ::url::{Host, Url};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use nmp_native_runtime_core::Cancellation;
use sha2::{Digest, Sha256};

use self::{
    address::is_public_ip,
    failure::{cancelled, failure, network_failure, svg_failure, too_large},
    mime::{SniffedMime, sniff_mime},
    url::{percent_decode, strip_scheme, validate_https_url},
};
use crate::{
    AcquiredResource, PinnedHttpsRequest, RasterizedSvg, ResolveRequest, ResourceClock,
    ResourceDeadline, ResourceErrorCode, ResourceFailure, ResourceNetwork, ResourceProviderLimits,
    SvgRasterRequest, SvgRasterizer,
};

pub(crate) use self::url::validate_blossom_server;

pub(crate) struct AcquisitionContext<'a> {
    pub network: &'a dyn ResourceNetwork,
    pub rasterizer: &'a dyn SvgRasterizer,
    pub clock: &'a dyn ResourceClock,
    pub limits: ResourceProviderLimits,
    pub blossom_servers: &'a [Url],
    pub cancellation: &'a Cancellation,
}

impl AcquisitionContext<'_> {
    pub fn acquire(&self, raw_url: &str) -> Result<AcquiredResource, ResourceFailure> {
        if raw_url.is_empty() || raw_url.len() > self.limits.maximum_url_bytes {
            return Err(failure(
                ResourceErrorCode::InvalidRequest,
                "resource URL is empty or exceeds its byte limit",
            ));
        }
        if self.cancellation.is_cancelled() {
            return Err(cancelled());
        }
        if let Some(encoded) = strip_scheme(raw_url, "data") {
            return self.acquire_data(encoded);
        }
        if let Some(address) = strip_scheme(raw_url, "blossom") {
            return self.acquire_blossom(address);
        }
        let url = Url::parse(raw_url).map_err(|_| {
            failure(
                ResourceErrorCode::InvalidRequest,
                "resource URL is not valid",
            )
        })?;
        if url.scheme() != "https" {
            return Err(failure(
                ResourceErrorCode::UnsupportedScheme,
                "only data, https, and canonical blossom URLs are supported",
            ));
        }
        let bytes = self.acquire_https(url)?;
        self.classify_and_sanitize(bytes)
    }

    fn acquire_data(&self, encoded: &str) -> Result<AcquiredResource, ResourceFailure> {
        let (metadata, body) = encoded.split_once(',').ok_or_else(|| {
            failure(
                ResourceErrorCode::InvalidRequest,
                "data URL is missing its comma separator",
            )
        })?;
        let base64 = metadata
            .split(';')
            .skip(1)
            .any(|token| token.eq_ignore_ascii_case("base64"));
        let bytes = if base64 {
            let compact = percent_decode(body)?
                .into_iter()
                .filter(|byte| !byte.is_ascii_whitespace())
                .collect::<Vec<_>>();
            STANDARD
                .decode(&compact)
                .or_else(|_| STANDARD_NO_PAD.decode(&compact))
                .map_err(|_| {
                    failure(
                        ResourceErrorCode::DecodeFailed,
                        "data URL base64 payload is invalid",
                    )
                })?
        } else {
            percent_decode(body)?
        };
        if bytes.len() > self.limits.maximum_response_bytes {
            return Err(too_large());
        }
        self.classify_and_sanitize(bytes)
    }

    fn acquire_blossom(&self, address: &str) -> Result<AcquiredResource, ResourceFailure> {
        let hash = address
            .strip_prefix("sha256:")
            .filter(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or_else(|| {
                failure(
                    ResourceErrorCode::InvalidRequest,
                    "Blossom URL must be blossom:sha256:<64 lowercase hex>",
                )
            })?;
        let mut last_failure = None;
        for server in self.blossom_servers {
            if self.cancellation.is_cancelled() {
                return Err(cancelled());
            }
            let target = server.join(hash).map_err(|_| {
                failure(
                    ResourceErrorCode::BlockedByPolicy,
                    "configured Blossom server could not form a safe URL",
                )
            })?;
            match self.acquire_https(target) {
                Ok(bytes) => {
                    if hex::encode(Sha256::digest(&bytes)) != hash {
                        return Err(failure(
                            ResourceErrorCode::DecodeFailed,
                            "Blossom response did not match its SHA-256 address",
                        ));
                    }
                    return self.classify_and_sanitize(bytes);
                }
                Err(error)
                    if matches!(
                        error.code,
                        ResourceErrorCode::NotFound | ResourceErrorCode::NetworkError
                    ) =>
                {
                    last_failure = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_failure.unwrap_or_else(|| {
            failure(
                ResourceErrorCode::NotFound,
                "no configured Blossom server returned the addressed bytes",
            )
        }))
    }

    fn acquire_https(&self, mut url: Url) -> Result<Vec<u8>, ResourceFailure> {
        let deadline = ResourceDeadline {
            monotonic_millis: self
                .clock
                .monotonic_millis()
                .saturating_add(self.limits.fetch_timeout_millis),
        };
        for redirect_count in 0..=self.limits.maximum_redirects {
            self.check_live(deadline)?;
            // URL fragments are client-side identifiers and must never cross
            // the raw network capability boundary.
            url.set_fragment(None);
            validate_https_url(&url)?;
            let host = url.host().ok_or_else(|| {
                failure(
                    ResourceErrorCode::InvalidRequest,
                    "HTTPS resource URL has no host",
                )
            })?;
            let port = url.port_or_known_default().ok_or_else(|| {
                failure(
                    ResourceErrorCode::InvalidRequest,
                    "HTTPS resource URL has no usable port",
                )
            })?;
            let host_text: Arc<str> = Arc::from(host.to_string());
            let addresses = match host {
                Host::Ipv4(address) => vec![IpAddr::V4(address)],
                Host::Ipv6(address) => vec![IpAddr::V6(address)],
                Host::Domain(_) => self
                    .network
                    .resolve(
                        &ResolveRequest {
                            host: Arc::clone(&host_text),
                            port,
                            deadline,
                        },
                        self.cancellation,
                    )
                    .map_err(network_failure)?,
            };
            if addresses.is_empty() {
                return Err(failure(
                    ResourceErrorCode::NetworkError,
                    "DNS resolution returned no addresses",
                ));
            }
            if addresses.len() > self.limits.maximum_resolved_addresses {
                return Err(failure(
                    ResourceErrorCode::TooLarge,
                    "DNS resolution exceeded its address limit",
                ));
            }
            if addresses.iter().any(|address| !is_public_ip(*address)) {
                return Err(failure(
                    ResourceErrorCode::BlockedByPolicy,
                    "DNS resolution included a non-public address",
                ));
            }
            let response = self
                .network
                .get(
                    &PinnedHttpsRequest {
                        url: Arc::from(url.as_str()),
                        host: host_text,
                        port,
                        approved_addresses: Arc::from(addresses),
                        maximum_body_bytes: self.limits.maximum_response_bytes,
                        deadline,
                    },
                    self.cancellation,
                )
                .map_err(network_failure)?;
            self.check_live(deadline)?;
            if response.body.len() > self.limits.maximum_response_bytes {
                return Err(too_large());
            }
            match response.status {
                200..=299 => return Ok(response.body),
                301 | 302 | 303 | 307 | 308 => {
                    if redirect_count == self.limits.maximum_redirects {
                        return Err(failure(
                            ResourceErrorCode::BlockedByPolicy,
                            "HTTPS redirect chain exceeded its limit",
                        ));
                    }
                    let location = response.location.as_deref().ok_or_else(|| {
                        failure(
                            ResourceErrorCode::NetworkError,
                            "HTTPS redirect omitted its Location",
                        )
                    })?;
                    if location.len() > self.limits.maximum_url_bytes {
                        return Err(failure(
                            ResourceErrorCode::TooLarge,
                            "HTTPS redirect target exceeded its byte limit",
                        ));
                    }
                    url = url.join(location).map_err(|_| {
                        failure(
                            ResourceErrorCode::BlockedByPolicy,
                            "HTTPS redirect target is invalid",
                        )
                    })?;
                }
                404 | 410 => {
                    return Err(failure(
                        ResourceErrorCode::NotFound,
                        "HTTPS resource was not found",
                    ));
                }
                _ => {
                    return Err(failure(
                        ResourceErrorCode::NetworkError,
                        "HTTPS resource returned an unsuccessful status",
                    ));
                }
            }
        }
        unreachable!("the bounded redirect loop always returns")
    }

    fn classify_and_sanitize(&self, bytes: Vec<u8>) -> Result<AcquiredResource, ResourceFailure> {
        if bytes.len() > self.limits.maximum_response_bytes {
            return Err(too_large());
        }
        let classification = sniff_mime(&bytes).ok_or_else(|| {
            failure(
                ResourceErrorCode::DecodeFailed,
                "resource bytes did not match a permitted MIME signature",
            )
        })?;
        if classification == SniffedMime::Svg {
            if bytes.len() > self.limits.maximum_svg_bytes {
                return Err(too_large());
            }
            let deadline = ResourceDeadline {
                monotonic_millis: self
                    .clock
                    .monotonic_millis()
                    .saturating_add(self.limits.svg_timeout_millis),
            };
            let rasterized = self
                .rasterizer
                .rasterize(
                    &SvgRasterRequest {
                        source: Arc::from(bytes),
                        maximum_dimension: self.limits.maximum_svg_output_dimension,
                        maximum_output_bytes: self.limits.maximum_response_bytes,
                        deadline,
                    },
                    self.cancellation,
                )
                .map_err(svg_failure)?;
            self.check_live(deadline)?;
            return self.validate_raster(rasterized);
        }
        Ok(AcquiredResource {
            bytes,
            mime: Arc::from(classification.mime()),
        })
    }

    fn validate_raster(
        &self,
        rasterized: RasterizedSvg,
    ) -> Result<AcquiredResource, ResourceFailure> {
        if rasterized.width == 0
            || rasterized.height == 0
            || rasterized.width > self.limits.maximum_svg_output_dimension
            || rasterized.height > self.limits.maximum_svg_output_dimension
            || rasterized.bytes.len() > self.limits.maximum_response_bytes
        {
            return Err(too_large());
        }
        let mime = match sniff_mime(&rasterized.bytes) {
            Some(SniffedMime::Png) => "image/png",
            Some(SniffedMime::Webp) => "image/webp",
            _ => {
                return Err(failure(
                    ResourceErrorCode::DecodeFailed,
                    "SVG rasterizer returned a non-PNG/WebP payload",
                ));
            }
        };
        Ok(AcquiredResource {
            bytes: rasterized.bytes,
            mime: Arc::from(mime),
        })
    }

    fn check_live(&self, deadline: ResourceDeadline) -> Result<(), ResourceFailure> {
        if self.cancellation.is_cancelled() {
            return Err(cancelled());
        }
        if self.clock.monotonic_millis() >= deadline.monotonic_millis {
            return Err(failure(
                ResourceErrorCode::Timeout,
                "resource acquisition timed out",
            ));
        }
        Ok(())
    }
}
