# Compatibility baseline report

Baseline `native-runtime-compat-v2` was captured on 2026-07-31. It is
machine-readable in [`compatibility.lock`](../compatibility.lock) and remains
**unratified**. Product-owner direction is recorded as `pablof7z`;
compatibility, security, and NMP-boundary review remain unsigned.

The old/new authority diff, behavior decisions, and migration status are in
[`compatibility-v2-upgrade.md`](compatibility-v2-upgrade.md). The v2 baseline
must not be described as accepted until the three blank signoffs are real.

## Pinned authorities

| Authority | Pin |
| --- | --- |
| NIP-5D | `nostr-protocol/nips#2303` head `eb45dfd7335b7f88cb53781984c553581d2b4c34` |
| NAP registry | `napplet/naps@5ac0490461ca6fec2f0d2e45b4835cf9bc08de24` |
| napplet packages | `napplet/web@60889f1c2476e063500c7ab6624af6abe0dbcbe5` plus lock-bound local compatibility patch |
| Kehto corpus | candidate `jodobear/kehto-web@62241de0b4526ba4fdc8a7b3c766c2499d3ae24d`, proposed to `kehto/web` |
| NMP | `pablof7z/nmp@005dc2a5f12aa414961b313d05ebb021934e385c` |

Package versions are `@napplet/core` 0.29.0, `@napplet/shim` 0.27.0,
`@napplet/sdk` 0.25.0, `@napplet/nap` 0.29.0, and
`@napplet/conformance` 0.14.0. The lock records both npm tarball SHA-256 values
and source-tree object IDs so published bytes and repository bytes are
independently pinned.

The local package changes are not represented as released or upstream bytes.
`compatibility.lock` binds `conformance/patches/napplet-web/compat-v2.patch` by
SHA-256, and baseline regeneration applies it to the clean exact commit before
verifying the resulting vendored evidence.

The clean remote NMP commit above remains the authority. Its Rust facade and
UniFFI component snapshots are separately hashed in the lock; no NMP API or
ownership boundary changed in this upgrade.

## Deliberate drift decisions

### Artifact shape

The pinned NIP-5D draft says a napplet is one self-contained `/index.html`.
The pinned `@napplet/vite-plugin` and existing ecosystem tooling accept both
`single-file` and `external-assets`. This baseline deliberately accepts both,
but only when every path hash, the required `/index.html`, and the aggregate
hash verify and every subresource is materialized from verified bytes.

This is compatibility support, not permission to navigate the iframe to a
remote URL or allow runtime network fetches.

### Redirected artifact acquisition

Artifact redirects are supported, but transport-library auto-follow is not.
For each approved source, Rust accepts only 301, 302, 303, 307, or 308 and
follows at most five hops. Every target is parsed again as a credential-free,
query-free, fragment-free HTTPS URL, resolved again, admitted only when every
reported address is public, and connected to only through those approved
addresses while preserving the target hostname for certificate validation and
SNI. Ambient proxy configuration is not used.

Each raw response must report the exact URL requested for that hop. Each
request has a finite byte ceiling and a 15-second default deadline. A public
redirect does not weaken artifact identity: no bytes become retained or
executable until every manifest path SHA-256 and the aggregate hash verify.
Unsafe targets, unapproved redirect statuses, missing locations, a sixth hop,
source confusion, deadline/byte exhaustion, and verification failure are
typed, observable refusals.

This is content addressing: the SHA-256 path hashes and aggregate hash are
themselves inside the signed manifest event, so the manifest's signature is
the provenance. Once bytes hash-match, which host or hop they physically
came from is not a security property to defend — that is precisely what
content addressing means. Do not re-litigate this as a "verify origin, not
just hash" regression; the hop/address/DNS-rebinding checks above exist to
bound *network egress and SSRF exposure* during acquisition, not to
re-establish origin trust that the hash already provides. See PR #62 and the
discussion on PR #76 (closed: "redirect following is intentional").

### NAP-SHELL

The pinned NAP registry defines `shell.ready` and `shell.init`. The pinned
`@napplet/core` domain union and `@napplet/conformance` envelope validator do
not include a `shell` domain, and the pinned shim explicitly installs no
generic shell object.

The baseline therefore tracks those two envelopes as
`registry-only-handshake`: they need a dedicated compatibility adapter and
validator before they can be advertised. Ordinary domain-object presence
remains the NIP-5D availability signal.

### Registry and package semantic drift

The pinned NAP-INC registry now requires a target-side
`inc.channel.opened` carrier and symmetric channel handles. The pinned 0.29
package types, shim, and conformance validator do not expose that carrier. The
inventory therefore records it as `explicit-unsupported`; this baseline does
not advertise symmetric channel attachment.

In the other direction, the 0.29 package line adds queryless normalized intent
requests and `intent.deliver`, while the pinned NAP-INTENT registry document
still describes the older optional-convention/INC-delivery model. The package
contract is the executable compatibility authority for those carriers, but it
does not make the registry text ratified. Provider promotion requires a
separate implementation review of the new delivery lifecycle.

### Registry and package domain breadth

The pinned registry contains NAP-IDENTITY, NAP-INC, NAP-INTENT, NAP-SHELL, and
NAP-THEME. The pinned `@napplet/nap` package exposes 22 domains. Eighteen
package domains therefore have no matching document in the pinned registry:
`relay`, `storage`, `keys`, `media`, `notify`, `config`, `resource`, `cvm`,
`outbox`, `upload`, `ble`, `webrtc`, `link`, `count`, `lists`, `serial`,
`common`, and `dm`.

The executable envelope inventory records all package-active types, but this
does not make the package-only domains ratified protocol. M0 advertises none.
Each future provider needs an explicit compatibility decision, package contract
tests, and platform matrix promotion.

### Conformance depth

Pinned `@napplet/conformance` checks kind/d-tag shape, a hashed `/index.html`,
known `requires`, sandbox/prelude boot, emitted envelope structure,
no-capability degradation, and listener teardown. It does not by itself prove
the event signature, every path hash, aggregate recomputation, duplicate
critical tags, bounded per-hop redirect revalidation, or full external-asset
closure.

The runtime compatibility gate is therefore the pinned suite **plus** the
stronger artifact, malicious-input, and native bridge tests in this repository.
Passing the upstream suite alone is never an installation verdict.

### Principal identity

NIP-5D defines the protocol identity as `(dTag, aggregateHash)`. The runtime
security principal is deliberately stronger:
`(manifestAuthor, dTag, aggregateHash)`. Protocol-visible identity remains
unchanged; the author is an internal grant/storage isolation dimension.

The pinned draft also permits snapshot kind `5129` and root kind `15129` while
forbidding a `d` tag on those kinds. That conflicts with its own mandatory
`(dTag, aggregateHash)` identity definition. The runtime therefore verifies
and caches those signed artifacts, but currently refuses to execute them with
the typed `unsupported_manifest_identity` reason. It does not silently invent
a dTag or let a caller choose one. Enabling those kinds requires an explicit
compatibility decision for a collision-free typed identity scope and all four
baseline signoffs. Named kind `35129` has an unambiguous signed dTag and is the
only executable manifest identity until then.

### NMP facade

The runtime uses NMP's public facade only. Current supported application nouns
are live queries and write intents/receipts, plus identity and diagnostics.
The runtime must not depend on mechanism crates or claim native Android support:
the pinned Kotlin package is desktop JVM, not a qualified Android AAR.

Known public gaps relevant to the product:

- no public receipt enumeration after a process loses an accepted receipt ID;
- Swift/Kotlin receipt consumption has no native observer-detach handle;
- public rows do not expose typed pending intent or receipt IDs;
- Swift/Kotlin omit some Rust configuration ceilings;
- secure standard Keychain/Keystore signer persistence is not shipped;
- scoped evidence must not be collapsed into global completeness.

## Upgrade gate

A baseline change is accepted only with:

1. a dedicated compatibility issue;
2. old/new NIP, NAP, package, corpus, and provider diffs;
3. regenerated source snapshots, envelope inventory, and corpus hashes;
4. an explicit newly-accepted/no-longer-accepted behavior list;
5. migration or dual-support behavior;
6. product, compatibility, security, and NMP-boundary signoff.

Patch or minor releases cannot silently drop a declared baseline.
