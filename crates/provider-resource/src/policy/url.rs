use url::Url;

use super::failure::failure;
use crate::{ResourceErrorCode, ResourceFailure};

pub(super) fn strip_scheme<'a>(raw_url: &'a str, expected: &str) -> Option<&'a str> {
    let (scheme, remainder) = raw_url.split_once(':')?;
    scheme.eq_ignore_ascii_case(expected).then_some(remainder)
}

pub(super) fn validate_https_url(url: &Url) -> Result<(), ResourceFailure> {
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

pub(super) fn percent_decode(encoded: &str) -> Result<Vec<u8>, ResourceFailure> {
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
