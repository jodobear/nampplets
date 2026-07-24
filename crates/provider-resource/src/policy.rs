use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use nmp_native_runtime_core::Cancellation;
use sha2::{Digest, Sha256};
use url::{Host, Url};

use crate::{
    AcquiredResource, PinnedHttpsRequest, RasterizedSvg, ResolveRequest, ResourceClock,
    ResourceDeadline, ResourceErrorCode, ResourceFailure, ResourceNetwork, ResourceNetworkError,
    ResourceProviderLimits, SvgRasterError, SvgRasterRequest, SvgRasterizer,
};

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

fn strip_scheme<'a>(raw_url: &'a str, expected: &str) -> Option<&'a str> {
    let (scheme, remainder) = raw_url.split_once(':')?;
    scheme.eq_ignore_ascii_case(expected).then_some(remainder)
}

fn validate_https_url(url: &Url) -> Result<(), ResourceFailure> {
    if url.scheme() != "https" {
        return Err(failure(
            ResourceErrorCode::BlockedByPolicy,
            "redirects and Blossom servers must remain HTTPS",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(failure(
            ResourceErrorCode::BlockedByPolicy,
            "credential-bearing HTTPS URLs are forbidden",
        ));
    }
    if url.host().is_none() {
        return Err(failure(
            ResourceErrorCode::InvalidRequest,
            "HTTPS URL has no host",
        ));
    }
    Ok(())
}

pub(crate) fn validate_blossom_server(raw: &str) -> Result<Url, ResourceFailure> {
    let mut url = Url::parse(raw).map_err(|_| {
        failure(
            ResourceErrorCode::InvalidRequest,
            "Blossom server URL is invalid",
        )
    })?;
    validate_https_url(&url)?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(failure(
            ResourceErrorCode::BlockedByPolicy,
            "Blossom server URL cannot contain a query or fragment",
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn percent_decode(encoded: &str) -> Result<Vec<u8>, ResourceFailure> {
    let input = encoded.as_bytes();
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            if index + 2 >= input.len() {
                return Err(failure(
                    ResourceErrorCode::DecodeFailed,
                    "data URL contains truncated percent encoding",
                ));
            }
            let high = hex_value(input[index + 1]);
            let low = hex_value(input[index + 2]);
            let (Some(high), Some(low)) = (high, low) else {
                return Err(failure(
                    ResourceErrorCode::DecodeFailed,
                    "data URL contains invalid percent encoding",
                ));
            };
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(input[index]);
            index += 1;
        }
    }
    Ok(output)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SniffedMime {
    Png,
    Jpeg,
    Gif,
    Webp,
    Avif,
    Svg,
}

impl SniffedMime {
    fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            Self::Svg => "image/svg+xml",
        }
    }
}

fn sniff_mime(bytes: &[u8]) -> Option<SniffedMime> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(SniffedMime::Png);
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(SniffedMime::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(SniffedMime::Gif);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(SniffedMime::Webp);
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"avif" | b"avis") {
        return Some(SniffedMime::Avif);
    }
    if looks_like_svg(bytes) {
        return Some(SniffedMime::Svg);
    }
    None
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let prefix = &bytes[..bytes.len().min(4_096)];
    let Ok(text) = std::str::from_utf8(prefix) else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    let lowercase = text.to_ascii_lowercase();
    lowercase.starts_with("<svg")
        || (lowercase.starts_with("<?xml")
            && lowercase
                .find("<svg")
                .is_some_and(|position| position < 1_024))
}

pub(crate) fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    if address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_multicast()
    {
        return false;
    }
    !matches!(
        octets,
        [0, ..]
            | [100, 64..=127, ..]
            | [169, 254, ..]
            | [192, 0, 0, ..]
            | [192, 0, 2, ..]
            | [192, 88, 99, ..]
            | [198, 18..=19, ..]
            | [198, 51, 100, ..]
            | [203, 0, 113, ..]
            | [240..=255, ..]
    )
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    if address.is_unspecified() || address.is_loopback() || address.is_multicast() {
        return false;
    }
    if (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
    {
        return false;
    }
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    true
}

fn network_failure(error: ResourceNetworkError) -> ResourceFailure {
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

fn svg_failure(error: SvgRasterError) -> ResourceFailure {
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

fn too_large() -> ResourceFailure {
    failure(
        ResourceErrorCode::TooLarge,
        "resource bytes exceeded the configured limit",
    )
}

fn cancelled() -> ResourceFailure {
    failure(
        ResourceErrorCode::NetworkError,
        "resource acquisition was cancelled",
    )
}

fn failure(code: ResourceErrorCode, message: &'static str) -> ResourceFailure {
    ResourceFailure::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ip_policy_blocks_local_special_and_documentation_ranges() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(
                !is_public_ip(address.parse().unwrap()),
                "{address} must be blocked"
            );
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn percent_decode_is_exact_and_refuses_malformed_input() {
        assert_eq!(percent_decode("a%20b").unwrap(), b"a b");
        assert!(percent_decode("%").is_err());
        assert!(percent_decode("%xx").is_err());
    }

    #[test]
    fn mime_sniff_never_trusts_labels_or_delivers_raw_svg() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\nx"), Some(SniffedMime::Png));
        assert_eq!(
            sniff_mime(b"<?xml version=\"1.0\"?><svg></svg>"),
            Some(SniffedMime::Svg)
        );
        assert_eq!(sniff_mime(b"<html>not an image</html>"), None);
    }
}
