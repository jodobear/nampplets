# ADR 0004: Keep runtime metadata separate from canonical NMP state

- Status: Accepted architecture; baseline signoff pending
- Date: 2026-07-24
- Invariants: I-03, I-04, I-09
- Requirements: FR-N01 through FR-N08, FR-R04, FR-R05

## Context

The runtime needs installations, grants, component storage, workspaces, and
activity, but NMP already owns canonical Nostr state, routing, signing, pending
rows, durable writes, receipts, provenance, and evidence. Copying those facts
would introduce two writers and divergent failure semantics.

## Decision

The runtime consumes the supported NMP public facade only. NMP does not know
about napplet sessions, WebViews, slots, or renderer catalogs.

The runtime store may contain verified installation records, artifact indexes,
grants/denials, exact-build KV/config, workspace definitions, handler choices,
bounded activity summaries, compatibility metadata, and crash recovery
metadata. It must not contain an authoritative event cache, replacement/deletion
model, relay truth, signer state, pending-row model, write state machine, or
receipt reconstruction.

One application profile maps to one runtime store and one NMP engine/store
trust domain. Mutually untrusted local users require separate profiles/stores.
Reset closes both owners before destructive work and distinguishes their data.

Rows retain scoped acquisition evidence and explicit shortfalls. EOSE, an empty
window, or offline cached data never becomes a global `synced`, `complete`, or
authoritative-empty fact.

## Consequences

- Runtime presentation caches must be disposable and mechanically derivable.
- Durable writes outlive originating components because NMP owns them.
- Receipt IDs must be persisted promptly; the pinned NMP facade cannot enumerate
  receipts after an application loses an accepted ID.
- Platform gaps cannot be bypassed with mechanism crates or raw UniFFI types.

## Verification

- Architecture dependency checks reject NMP mechanism imports.
- Restart reattaches the persisted receipt and preserves frozen author/body.
- Component destruction does not cancel an accepted durable write.
- Runtime-store schema audits find no canonical Nostr event/write ownership.
