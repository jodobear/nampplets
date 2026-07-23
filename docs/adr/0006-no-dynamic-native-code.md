# ADR 0006: Exclude dynamic native code and protocol cartridges from v1

- Status: Accepted
- Date: 2026-07-24
- Invariant: I-06

## Context

Loading Swift, Kotlin, Rust, dynamic libraries, native plug-ins, or protocol
cartridges would create a different trust, packaging, upgrade, and store-review
model from sandboxed verified WebView presentation.

## Decision

Version 1 dynamically installs only verified HTML, JavaScript, CSS, and related
web assets executed in the napplet sandbox. Native runtime packages and NMP
protocol modules ship with the signed native application. Dynamic WASM services
or protocol cartridges require a separate future product/ADR after the
WebView-hosted model is proven.

## Consequences

- A napplet cannot extend NMP mechanism code at runtime.
- New native providers or protocol modules require an application release.
- The artifact verifier rejects executable native-library shapes.

## Verification

Artifact fixtures containing native libraries or unsupported executable modes
are rejected before cache or execution.
