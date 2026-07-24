# Catalog resolver

This crate owns the bounded policy between a user-supplied manifest coordinate
and the existing sealed artifact resolver. It deliberately does not own relay
connections, Nostr replacement semantics, DNS, sockets, or artifact bytes.

Production integration has three injected boundaries:

- `ManifestLookupPort` is implemented beside the one application-owned NMP
  engine. It observes the coordinate through the public `nmp::Engine` facade,
  lets NMP select the canonical snapshot/replaceable row, and returns that one
  raw signed event plus scoped per-source evidence. This crate never opens a
  relay, sorts replaceable events, or claims global completeness.
- `HttpsAcquisitionPort` executes one raw HTTPS request with redirects disabled
  and a streaming `maximum_bytes + 1` cap. It reports the exact effective URL,
  status, redirect metadata, resolved socket addresses, and bounded response
  bytes. Rust in this crate decides whether those raw facts are safe.
- `SealedArtifactCache` indexes already verified `VerifiedArtifactHandle`
  values by aggregate hash. A persistent implementation belongs with
  `crates/artifact`; it must reopen only artifact-owned immutable bytes. The
  included memory implementation is a bounded deterministic implementation for
  tests and process-local use, not a second artifact store.

The resolver refuses HTTPS targets that resolve to loopback, private,
link-local, multicast, documentation, benchmarking, reserved, or unspecified
addresses. Redirects, an effective URL different from the requested candidate,
oversize bodies, missing DNS evidence, and source confusion are refused before
the artifact cache can commit. Transport and HTTP availability failures may
advance to the next finite approved Blossom candidate; security-policy
refusals fail the whole acquisition.

Cancellation is cooperative and event-driven: the same token is passed to both
ports and checked at every ownership boundary. There are no polling or
sleep-check loops. Concurrent operations, lookup facts, acquisition facts,
resolved addresses, URLs, strings, event bytes, cached entries, and cached
aggregate bytes all have explicit finite limits.

Architecture discharge:

- D2/D3/D10: NMP remains the only relay, history, routing, and private-source
  owner.
- D4: NMP selects the manifest row; `crates/artifact` remains the only artifact
  verifier and byte owner.
- D7: injected transport reports raw network facts; Rust owns policy.
- D8: admission is zero-queue and every collection is bounded.
- D9: this layer has no wall-clock or replacement-time policy.
