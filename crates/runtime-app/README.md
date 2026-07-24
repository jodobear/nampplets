# Runtime application kernel

This crate is the Rust-owned composition root. Platform shells submit semantic
commands and render bounded snapshots/events; they do not choose session
identity, provider policy, NMP routing, receipt ownership, or recovery behavior.

## Manifest identity gap

The pinned compatibility baseline accepts NIP-5D kinds 5129, 15129, and 35129,
while the current runtime principal contract is `(manifest author, dTag,
aggregateHash)`. A verified kind-35129 named manifest supplies all three fields.
Verified kind-5129 snapshot and kind-15129 root manifests correctly have no
`d` tag, and the baseline does not yet specify a typed runtime-principal mapping
for their snapshot/root coordinates.

The kernel therefore reports `UnsupportedManifestIdentity` for verified 5129
and 15129 artifacts before installation. It does not accept a caller-selected
surrogate or silently synthesize an untyped `dTag`. Supporting those kinds
requires a compatibility decision that defines a verifier-produced typed
snapshot/root identity and migrates the principal, grant, store, and activity
projections together.

## Installed library and uninstall ownership

The runtime store persists a bounded exact-build library. On restart, verified
title/manifest metadata is immediately listable and searchable, while launch
availability remains `MetadataOnly` until the artifact owner reattaches an
immutable verifier-produced handle for the same aggregate. A launch never
resolves mutable bytes from library metadata.

`RuntimeOwnedExactBuildState` uninstall stops only sessions for that principal
and atomically removes its installation row, grants, component KV, and explicit
workspace assignments. It preserves workspace definitions, retained NMP
receipt identifiers, activity evidence, and every NMP-owned canonical event,
pending write, route, receipt, and outcome. The artifact cache does not yet
expose an exact-build deletion API, so sealed-byte reclamation is an explicit
integration gap rather than an unsafe cross-owner deletion.

## Exact-build permission transactions

Every installed build persists a finite typed capability-request inventory.
`RuntimeApp::permission_review` joins that inventory with registered provider
metadata, the live grant ledger, and durable rows without exposing provider or
store internals to native code. Missing provider metadata remains explicitly
unknown and cannot be allowed.

`PlatformCommand::ApplyPermissionBatch` requires exactly one decision for every
requested capability. The kernel validates duplicates, unknown domains,
provider availability, dependency closure, managed-policy ownership, and the
exact principal before committing. Required-domain denial is a valid user
decision and remains durable; the separate launch command then refuses before
execution because negotiation cannot satisfy the required set.

The grant ledger and SQLite transaction commit as one owner operation.
Irreversible resource cancellation, provider-push revocation, activity facts,
and `PermissionBatchApplied` are emitted only after persistence succeeds.
Launch is never part of the permission transaction.
