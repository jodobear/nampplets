# Sandwich interview synthesis

Source: `30: Napplets w/ Sandwich`, published 2026-07-22:
<https://sovereignengineering.io/podcast/30-napplets-w-sandwich>

This is a timestamped synthesis of the diarized transcript. It captures author
intent and experience, not normative protocol. The transcript contains speech
recognition variants such as "naplet", "Noster", and "Keeto"; this reference
normalizes obvious names without treating ambiguous words as facts.

## Product motivation

- `01:04-02:17` — Standard clients were seen as inherently limited by one team
  rebuilding keys, relays, caching, outbox, and every feature in a silo. The
  proposed alternative is atomic functionality that can be switched and
  composed across clients.
- `03:28-05:59` — The visual idea drew from a composable desktop/window-manager
  mindset. Early prototypes revealed main-thread pressure, repetition, and
  security problems.
- `25:12-25:38` — A napplet is described as a hosted application that does one
  thing well. The single-file form was intended to reinforce focus and
  portability.

Advisor consequence: start product advice from focused jobs and host
composition, not from protocol envelopes or Kehto packages.

## The boundary emerged through subtraction

- `05:59-07:40` — Early iframes still had the runtime's authority. Iteration
  progressively removed features until the frame could essentially only talk
  over `postMessage`.
- `07:40-09:32` — Recreating all of Nostr inside the sub-protocol was the wrong
  abstraction. The napplet needs high-level helpers plus lower-level escape
  hatches: outbox-aware retrieval is a helper; explicit relay access is the
  escape hatch.
- `12:53-14:14` — An earlier design used a relay-like shell and assigned a key
  pair to each napplet. It preserved attribution but added unnecessary
  authentication and routing complexity because the runtime already controlled
  the session.
- `14:15-15:06` — Resource and media problems led to an operating-system analogy:
  reuse mature capability patterns instead of inventing Nostr-specific answers
  for every host service.

Advisor consequence: protect the seam. Do not give the frame ambient authority
or turn every host capability into raw Nostr messaging.

## Runtime mediation

- `15:16-17:59` — An upload napplet requests "upload"; the runtime chooses
  configured backends, destinations, prompts, and later resolution. The author
  names Blossom, HashTree, IPFS, and even Google Drive to show that the napplet's
  intent can be independent of provider policy.
- `18:59-21:04` — Signing, encryption, and decryption belong beyond the shell.
  Napplet-to-runtime content is intended to remain inspectable cleartext; the
  runtime cannot protect a user from behavior it cannot see.
- `29:26-30:40` — Removing direct web APIs, using CSP/sandboxed iframes, keeping
  the main thread for rendering/routing, and moving work to workers were
  described as both security and performance controls. Without runtime pressure
  control, one napplet could overwhelm relays or crash the parent.

Advisor consequence: capabilities need policy, consent, pressure limits,
inspection, and provider abstraction. "It works" without those is incomplete.

## Tooling and implementation

- `21:38-24:59` — Kehto was described as runtime packages; Paja as a workbench;
  separate napplet tooling as SDK, shim, types, boilerplate, skills, and
  conformance help. Minimal dependencies were an intentional security and
  maintenance choice at interview time.
- `10:21-11:22` and `49:42-50:33` — The workshop demonstrated very fast creation
  and publication, including agent-assisted authoring. These are event
  observations, not a guaranteed current onboarding time.
- `37:03-37:22` — Integration drift was acknowledged: existing clients had not
  all chased the moving spec.

Advisor consequence: explain the whole ecosystem and version it. Never equate
an impressive workshop or one successful runtime with general conformance.

## Web, native, and permissionless resolution

- `31:30-33:34` — Browser delivery was chosen partly because a user can open a
  link without installing anything. The runtime was described as resolving
  manifests/blobs itself and validating hashes rather than relying on a gateway,
  enabling permissionless and offline-capable behavior.
- `33:53-36:13` — The browser was intentionally treated as one difficult edge.
  A native app, browser/explorer, operating system, and RISC-V microkernel
  experiment were used to test whether the capability contracts could survive
  radically different environments.

Advisor consequence: distinguish the web projection from the transport-neutral
capability seam. A native runtime is not "Kehto rewritten"; it should preserve
contracts while changing trusted execution and transport.

## Inter-napplet composition

- `09:34-10:21` — Cross-napplet communication was one of the hardest problems,
  and showing the paradigm proved more effective than explaining it abstractly.
- `39:35-43:27` — The working direction used an
  `applet:<archetype>/<intent>`-style scheme analogous to mobile intents.
  Payload semantics were deliberately exploratory and sometimes inferred from
  context. The speakers valued agent legibility and real independent
  interoperability over premature exhaustive standardization.
- `43:57-45:48` — The seam is initially jarring to Nostr developers. A host
  runtime can be a translation layer over existing business logic, while a
  napplet author needs less Nostr infrastructure knowledge and leaves security
  and optimization to runtime specialists.

Later NAP work changed vocabulary and made conventions/archetypes more explicit.
Use the current NAP registry, not the interview's exact URI spelling or
underspecified payload rules.

## Many runtime shapes

- `45:52-48:22` — Runtimes can be minimal, social, game/mod systems, global media
  hosts, collaboration tools, editors, or plugin systems. Napplets need not all
  be social, and a runtime may expose a very small capability set.
- `48:22-51:04` — A kitchen-sink client can be decomposed into replaceable
  surfaces. Clipboard and ordinary user workflows can remain useful composition
  mechanisms. The same artifact behaving in another runtime was treated as a
  key success signal.

Advisor consequence: evaluate a runtime by truthful policy and portability, not
by how much desktop chrome or how many domains it has.

## Relay and ecosystem context

- `59:12-1:03:14` — Work on nsites, NIP-46 tooling, gateways, Blossom, deployers,
  and spec migrations provided practical background for artifact publication
  and adoption. Shipping a complete reference stack was described as a way to
  move an ecosystem toward a revised spec.
- `1:03:14-1:09:27` — Hard-coded relays were criticized. Outbox solves much but
  not all discovery. NIP-66 observations and subjective trust can help rank and
  optimize relay choices.
- `1:11:53-1:14:29` — Relay attributes were framed as an intentionally emergent
  vocabulary that lets users and software discover specialized relays without
  hard-coded lists.

Advisor consequence: higher-level runtime domains should own evolving relay
discovery and routing policy. Napplet code should not freeze a global relay
strategy unless its product is explicitly relay-local.

## Engineering method

- `1:19:32-1:20:31` — The author describes starting with a high-level spec,
  expanding and compressing it, then breaking it into focused low-level design
  documents suitable for delegated implementation.
- `1:23:53-1:25:56` — Novel domains require reviewing generated research rather
  than trusting model priors; a Socratic planning loop can surface missing
  assumptions.
- `1:27:23-1:32:30` — Strict repository/PR policy, issue refinement, independent
  review, anti-slop checks, TDD/BDD, and model diversity were described as ways
  to keep code from poisoning future context and tests from merely confirming
  an implementation after the fact.

Advisor consequence: recommendations should become executable contracts,
negative tests, and independent review, especially in an alpha ecosystem.

## What not to infer

The interview does not prove:

- the current NIP-5D or NAP wire format;
- present package exports or domain status;
- current interoperability of named clients;
- a universal single-file rule outside a selected spec revision;
- that Kehto is the canonical runtime;
- that every implicit convention will interoperate;
- that direct relay access, gateway behavior, or native projections are settled.

Use it to understand the "why," then verify the "what" live.
