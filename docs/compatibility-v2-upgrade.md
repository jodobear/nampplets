# Native runtime compatibility v2 upgrade report

Corrected and recaptured 2026-07-31. This is a candidate compatibility change from
`native-runtime-compat-v1` to `native-runtime-compat-v2`; it is not ratified.
`compatibility.lock` is the machine-readable authority.

## Authority changes

| Authority | v1 | v2 candidate | Material change |
| --- | --- | --- | --- |
| NIP-5D PR 2303 | `78efc118278e3ed42201eba9b60530b65835d7ed` | `eb45dfd7335b7f88cb53781984c553581d2b4c34` | strict child CSP guidance added |
| NAP registry | `6461e4b37c29dc09a20dff35d9515889c4433874` | `5ac0490461ca6fec2f0d2e45b4835cf9bc08de24` | convention URI transposition and symmetric INC channel semantics added |
| napplet/web | `b335c40c77f55547f23af81d6d999e2e4e3a3623` | `60889f1c2476e063500c7ab6624af6abe0dbcbe5` | core/nap 0.29, shim 0.27, SDK 0.25, conformance 0.14 |
| NAP-LISTS | absent from the v1 evidence | `napplet/naps@2c244b57051285178bcbb72e75ab2cbc8a3f8785` via PR 68, with released binding from `napplet/web` PR 85 merge `086f36e40d2df86349ece181dbe52660ba49090e` | exact semantic selectors, item names, support records, and error envelopes bound to released package source |
| NIP-51 type table | implicit | `nostr-protocol/nips@375160536f1ea9fac88708b9fd8464a60e3e7ad8` | derived list type names and supported public item classes pinned instead of guessed |
| Kehto corpus | `kehto/web@bb3929b3523b75356fd65f658f9bd14c7ff697e4` | `jodobear/kehto-web@62241de0b4526ba4fdc8a7b3c766c2499d3ae24d` | fork branch `fix/napplet-conformance-no-modulepreload` disables module preload so production artifacts contain no forbidden fetch helper; upstream merge is not assumed |
| NMP | `005dc2a5f12aa414961b313d05ebb021934e385c` | unchanged | no facade or ownership change |

All source-tree IDs, npm tarball SHA-256 values, vendored authority snapshots,
envelope inventory, and Kehto source-corpus hashes were regenerated from clean
worktrees at those exact revisions.

## Newly accepted package behavior

- `inc.emit` cannot carry a caller-supplied `sender`; delivered `inc.event`
  requires a runtime-attested sender.
- The 0.29 web binding transposes supported convention URI query parameters to
  a queryless identity plus text payload before the carrier crosses the trust
  boundary.
- `intent.invoke.request` carries matching `archetype`, `action`, and queryless
  `convention` fields.
- `intent.deliver` is an independent shell-to-napplet carrier with
  runtime-attested sender provenance.
- The NIP-5D source snapshot now recommends a strict self-contained iframe CSP.
- NAP-LISTS uses exactly one `kind` or derived `type`, semantic `itemType`
  values such as `pubkey`, package-shaped support records, and machine-readable
  error codes with a separate human `reason`.
- The Rust LISTS catalog derives exact names from the pinned NIP-51 table and
  advertises only public item types its adapter can preserve and encode.
- Public mutations preserve unrelated tags and encrypted content. Explicit
  private items, relay/label hints, and metadata options receive typed refusals
  because this implementation cannot fulfill them without inventing encoding
  or encryption behavior. A removal with omitted visibility fails closed when
  encrypted content exists because the draft contract requires matching both
  public and private items.

## No-longer-accepted package behavior

- A napplet-emitted `inc.emit.sender` is rejected.
- A 0.29 intent request with a missing identity field, a queried or fragmented
  convention, mismatched archetype/action, or caller-supplied sender is rejected
  by the pinned conformance validator.
- Kehto production artifacts containing Vite's module-preload `fetch` helper
  are not compatibility fixtures.
- Raw NIP-51 tags such as `{ "type": "p" }`, kind-only selector handling, and
  human prose in the `error` field are rejected as a false package contract.

## Explicitly unsupported registry behavior

The new NAP-INC text requires `inc.channel.opened` and symmetric target-side
channel handles. The 0.29 package types, shim, and conformance validator do not
ship that carrier. It is recorded as `explicit-unsupported` in the envelope
inventory and is not advertised by this baseline.

The NAP-INTENT text still describes the older optional-convention and
INC-delivery model, while the released package implements normalized convention
identity and `intent.deliver`. The executable package contract is pinned, but
this registry/package disagreement remains a ratification risk.

NAP-LISTS itself remains a draft proposal. The released 0.29 package provides
an executable web binding, but this baseline does not call the proposal
ratified and does not advertise private-item support.

## Migration and dual support

Existing 0.28-built napplets remain source-compatible at the current provider
boundary: legacy optional intent fields and the historic `protocol` alias are
still accepted by the Rust provider. The stricter 0.29 web binding emits the
normalized form. This tolerance is one-way; the runtime never emits a forged
sender or converts a 0.29 carrier back into caller-controlled identity.

No platform advertises a domain at M0. Promoting `intent` requires a separate
provider change that implements and tests independent target delivery; changing
the compatibility pin alone does not make that lifecycle complete.

## Acceptance state

- Product-owner direction: `pablof7z` (carried forward).
- Compatibility review: missing.
- Security review: missing.
- NMP-boundary review: missing.

The candidate remains `unratified` until those named reviews are recorded.
