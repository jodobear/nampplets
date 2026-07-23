# ADR 0001: Pin legacy compatibility before runtime expansion

- Status: Accepted architecture; baseline signoff pending
- Date: 2026-07-24
- Invariants: I-01, I-02
- Requirements: FR-C01 through FR-C10

## Context

NIP-5D, the NAP registry, packages, conformance harness, and real ecosystem
hosts evolve independently. Following their default branches would make
runtime behavior depend on build time. Implementing the private surface model
before legacy behavior is executable would prove a different product.

The pinned NIP draft describes a self-contained `/index.html`, while the pinned
package/tooling baseline supports both single-file and external-asset builds.

## Decision

`compatibility.lock` pins every authority, package version/source tree, accepted
manifest kind, artifact mode, WebView rule, and platform provider list.

Existing conformant napplets are a hard contract and become green before the
surface extension is implemented or marketed. macOS is the first reference
platform. Both pinned package artifact modes are accepted, but external assets
must be fetched, individually verified, aggregate-verified, cached immutably,
and materialized through a private non-network source. Unsupported providers
are absent and never simulated.

A baseline update requires a dedicated compatibility change, regenerated
fixtures/inventories/reports, an explicit behavior diff, migration policy, and
four-role signoff.

## Consequences

- A moving upstream draft cannot silently change released behavior.
- Compatibility work carries an explicit dual-mode artifact obligation.
- The private surface protocol cannot be used to disguise incomplete legacy
  conformance.
- Dropping a supported baseline is a major product change with deprecation.

## Verification

- Lock schema and upstream pins are tested.
- Vendored source snapshots and corpus bytes have deterministic digests.
- The envelope inventory covers every pinned type or records it as explicitly
  unsupported.
- One-byte fixture mutation must fail the integrity gate.
