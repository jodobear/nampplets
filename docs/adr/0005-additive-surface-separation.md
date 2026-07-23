# ADR 0005: Make surfaces additive renderers over host-owned bindings

- Status: Accepted architecture; baseline signoff pending
- Date: 2026-07-24
- Invariants: I-01, I-05, I-10
- Requirements: FR-S01 through FR-S10

## Context

The product needs replaceable UI modules without making WebViews authoritative
for application or Nostr state. Existing napplets must remain ordinary NIP-5D
artifacts even if the private surface extension is never standardized.

## Decision

Surface capability is optional and additive. A descriptor-less napplet is
legacy. A surface descriptor is inert, finite, versioned JSON embedded in and
hashed with verified `/index.html`; it grants nothing.

Surface v1 uses host-defined typed bindings, never arbitrary component-defined
NMP demand. Renderer profile receives no relay, query, or outbox domain by
default. Hybrid profile is selected by host policy before launch; a component
cannot escalate its session.

Bindings belong to workspace slots rather than WebViews. Native and web
renderers may consume one binding. Replacement mounts the new renderer, sends
the latest snapshot, waits for readiness, atomically switches presentation,
then unmounts the old renderer without restarting binding/NMP demand.

State is versioned per input port. Snapshots establish state; deltas name exact
from/to revisions; a gap requests resynchronization. Slow consumers converge by
latest bounded snapshot or bounded composed transition, not a growing queue.

Actions are declared, namespaced, versioned, schema-validated, policy-checked,
attributed, bounded, and routed by the host. They are typed intent, not arbitrary
commands. Legacy fallback is explicit and does not redefine NIP-5D.

## Consequences

- The first surface implementation waits for legacy conformance.
- WebView crashes do not destroy workspace-owned NMP demand.
- The host owns navigation, application state, signing approval, and action
  routing.
- Surface schema growth requires a small versioned host registry and fixtures.

## Verification

- Descriptor-less artifacts receive no surface domain.
- A renderer cannot invoke undeclared actions or outbox without grant/profile.
- Revision gaps resynchronize instead of applying a corrupt delta.
- Renderer A -> B preserves binding and NMP observation identity.
