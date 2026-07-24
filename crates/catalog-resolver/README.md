# Catalog resolver

This crate owns the bounded policy between a user-supplied manifest coordinate
and the existing sealed artifact resolver. It deliberately does not own relay
connections, Nostr replacement semantics, DNS, sockets, or artifact bytes.

Production integration has three Arc-owned boundaries:

- `ManifestLookupPort` is implemented beside the one application-owned NMP
  engine. It observes the coordinate through the public `nmp::Engine` facade,
  lets NMP select the canonical snapshot/replaceable row, and returns that one
  raw signed event plus scoped per-source evidence. Lookup is a nonblocking
  start/completion/operation contract: external cancellation wakes the
  resolver and synchronously signals the owned NMP observation cancel handle.
  This crate never opens a relay, sorts replaceable events, or claims global
  completeness.
- `HttpsAcquisitionPort` has the same nonblocking
  start/completion/operation contract. `RustHttpsAcquisitionPort` is the
  production implementation: one fixed-size Rust runtime, zero-queue
  admission, system DNS, public-address admission before connect, exact
  approved-address pinning under the original hostname (preserving TLS/SNI),
  redirects disabled, one overall deadline, and a streaming
  `maximum_bytes + 1` cap. It reports the exact effective URL, status, redirect
  presence, resolved socket addresses, and bounded response bytes for a second
  defensive validation before retention.
- `SealedArtifactCache` indexes already verified `VerifiedArtifactHandle`
  values by aggregate hash. A persistent implementation belongs with
  `crates/artifact`; it must reopen only artifact-owned immutable bytes. The
  included memory implementation is a bounded deterministic implementation for
  tests and process-local use, not a second artifact store.

`begin_review` verifies and freezes the exact signed selected manifest,
coordinate, event id, aggregate, and scoped evidence in an opaque
`ArtifactReview`. `confirm_review` consumes that token exactly once and never
re-runs lookup, so a newer replacement cannot retarget an approval. Cancel,
drop, and confirm all release review ownership; the default resolver admits at
most 16 pending reviews and returns typed saturation, stale, and foreign-token
refusals.

The resolver refuses HTTPS targets that resolve to loopback, private,
link-local, multicast, documentation, benchmarking, reserved, or unspecified
addresses. Redirects, an effective URL different from the requested candidate,
oversize bodies, missing DNS evidence, and source confusion are refused before
the artifact cache can commit. Transport and HTTP availability failures may
advance to the next finite approved Blossom candidate; security-policy
refusals fail the whole acquisition.

Cancellation is cooperative and event-driven: a bounded wake registration
unblocks the resolver immediately, then the resolver cancels the exact port
operation. There are no polling or sleep-check loops and no per-request
unbounded thread. Concurrent operations, pending reviews, cancellation wakes,
lookup facts, acquisition facts, resolved addresses, URLs, strings, event
bytes, cached entries, and cached aggregate bytes all have explicit finite
limits.

Artifact HTTPS is ordinary networking and therefore remains Rust-owned under
the repository's NMP/RMP rules. It is not a native OS-capability callback:
native code still owns bounded execution of capabilities such as Keychain,
file pickers, and opening user-approved links, while Rust owns DNS, transport,
SSRF/redirect policy, deadlines, cancellation, and acquisition error semantics.

Architecture discharge:

- D2/D3/D10: NMP remains the only relay, history, routing, and private-source
  owner.
- D4: NMP selects the manifest row; `crates/artifact` remains the only artifact
  verifier and byte owner.
- D7: the NMP adapter executes only the public observation and reports exact
  rows/evidence; Rust owns lookup lifecycle and every HTTPS policy decision.
- D8: admission is zero-queue and every collection is bounded.
- D9: this layer has no wall-clock or replacement-time policy.
