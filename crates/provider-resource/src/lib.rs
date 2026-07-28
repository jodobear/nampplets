//! Hardened, bounded NAP-RESOURCE byte broker.
//!
//! The provider never gives a napplet a network primitive. Rust validates the
//! exact URL, DNS answers, redirect chain, response size, MIME signature,
//! Blossom digest, SVG raster result, rate/concurrency quota and lifecycle.
//! The injected [`ResourceNetwork`] and [`SvgRasterizer`] execute only the raw
//! bounded capabilities Rust requests.
//!
//! JSON has no byte-string type, so the native provider envelope represents
//! the NAP `bstr` `blob` field as standard padded base64. The trusted web
//! projection must turn that field into a `Blob` before posting the terminal
//! envelope into the sandbox. The untrusted napplet never observes the base64
//! transport representation.

mod policy;
mod provider;
mod shared;
mod types;
mod wire;

pub use provider::ResourceProvider;
pub use types::*;
pub use wire::ResourceCensus;

pub const DOMAIN: &str = "resource";
pub const PINNED_NAP_PROTOCOL: &str = "napplet-web@0.29.0";
pub const NATIVE_BLOB_ENCODING: &str = "base64-standard-padded";

#[cfg(test)]
mod tests;
