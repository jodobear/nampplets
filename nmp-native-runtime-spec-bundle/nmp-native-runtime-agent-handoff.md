# NMP Native Runtime — Agent Handoff

## Read first

1. `nmp-native-napplet-runtime-product-spec.md`
2. `nmp-native-runtime-core-bdd.feature`
3. The repository’s future `compatibility.lock`
4. NMP’s public-facade and ownership documentation
5. The pinned NIP-5D, NAP, napplet SDK/conformance, and Kehto sources recorded by M0

## Product sentence

Build a native NMP-backed runtime that runs existing NIP-5D napplets unchanged and adds an optional surface profile for replaceable WebView renderers over host-owned state.

## The ordering rule

Do not implement the surface extension first.

The required order is:

1. pin compatibility;
2. prove the native WebView trust boundary;
3. pass existing napplet conformance unchanged;
4. add native providers and NMP integration;
5. add host-owned bindings and surface actions;
6. build the product vertical slice;
7. expand domains and harden.

A surface demo built before legacy compatibility is green is not progress against this product.

## Non-negotiable boundaries

- Existing conformant napplets require no source or build change.
- The untrusted iframe never receives the native bridge, `window.nostr`, keys, or direct network.
- The trusted shell identifies messages by `MessageEvent.source` and forwards an opaque native-created session.
- NMP remains the canonical Nostr store, query engine, and write owner.
- Runtime storage contains installation, grant, workspace, and scoped component data—not a second Nostr truth.
- Unsupported NAP domains are not advertised.
- The surface profile is optional and additive.
- Renderer replacement preserves the workspace binding and NMP demand.
- Every queue, stream, subscription, and resource class is finite and observable.

## First implementation batch

The first batch should produce only M0 outputs:

- repository skeleton and dependency checks;
- completed compatibility lock with exact versions/commits;
- imported conformance fixtures;
- immutable real-napplet corpus;
- threat model;
- ADRs for principal, bridge, compatibility, storage, and surface separation;
- deterministic relay/blob/signer test services;
- failing BDD/falsifier tests for the core invariants;
- empty native Workbench shell.

Do not guess around spec drift. Record it in the compatibility report and select a pinned behavior deliberately.

## Workstream isolation

Parallelize compatibility, WebView security, runtime core, NMP adapter, native product, and adversarial QA only after shared contracts are ratified. One agent owns each public state machine or schema. A second independent agent attacks it.

## Issue completion rule

An issue is complete only when:

- its BDD scenario passes;
- failure and teardown are tested;
- compatibility impact is recorded;
- resource ownership returns to baseline;
- diagnostics exist for refusals and sensitive operations;
- no capability is advertised prematurely;
- no parallel Nostr truth is introduced;
- release-build behavior is tested where WebView/CSP/FFI is involved.

## First product proof

The first meaningful demo is not “a napplet renders in WKWebView.” It is:

1. an existing napplet runs unchanged;
2. a host-owned NMP feed binding is rendered by surface A;
3. surface A is replaced by surface B;
4. the binding and NMP observation do not restart;
5. a native view sees the same state;
6. a composer creates a durable NMP write;
7. the composer is destroyed;
8. the write and pending row continue and survive restart.
