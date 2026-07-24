# ADR 0007: Pin the Good Morning capability profile to its exact build

## Status

Accepted for the unratified native-runtime compatibility baseline.

## Context

The published Good Morning artifact is an immutable accepted fixture. Its
signed kind-35129 event predates NIP-5D `requires` tags, while its sealed
application bytes and the native catalog consistently identify identity, INC,
and outbox as essential and resource, theme, and link as optional.

NIP-5D permits an artifact with no `requires` tags to load with capabilities
the shell provides. That does not authorize Swift to invent requirements,
auto-grant a sensitive domain, or launch as a side effect of installation.
The Workbench may opt into a clearly named, isolated demo mode, but that mode
must remain Rust-owned and exact-triple guarded.

## Decision

`compatibility.lock` records one exact profile keyed by the complete verified
identity `(manifest author, dTag, aggregateHash)`. Rust adds that profile to
the installed capability-request inventory only when all three values match:

- required: `identity`, `inc`, `outbox`;
- optional: `resource`, `theme`, `link`.

Signed `requires` tags remain authoritative for every general artifact.
Native code cannot select a compatibility profile or pass capability names
into installation. Rust persists the derived request inventory, owns the
atomic decision batch, and rederives required launch domains from the sealed
artifact identity. Install, permission application, and launch remain three
separate operations. A dedicated `DemoPinnedGoodMorning` runtime mode may
submit that same atomic batch after installation, only for the complete pinned
identity; it grants only registered available providers and explicitly denies
unknown or unavailable optional domains. The normal `Interactive` mode never
does this.

Any publisher, dTag, or aggregate change is a different principal, receives no
profile match, and inherits no grant. The profile does not claim that an
unregistered provider is supported: permission review reports such domains as
unknown and allows denial only.

## Consequences

- The unchanged published artifact can receive a truthful exact-build review
  without changing or re-signing its bytes.
- Outbox remains sensitive and cannot be granted by the normal Workbench
  startup path; the isolated demo mode is the sole exact-fixture exception.
- Resource and link stay absent until real bounded native executors are
  registered.
- A future Good Morning release with signed `requires` tags must use its new
  exact identity; retiring or changing this exception is a deliberate
  compatibility change.
