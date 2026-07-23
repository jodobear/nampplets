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
