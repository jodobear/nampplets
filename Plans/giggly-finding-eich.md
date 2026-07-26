# NAP-INTENT: wire real archetype-based intent dispatch end-to-end

## Context

The `/goal` for this session requires that clicking a nip29 group in the
`nip29-groups` napplet actually opens the `nip29-chat` napplet on that group,
**through the real NAP-INTENT capability layer** — no hardcoded routing. The
policy kernel for this already exists at `crates/provider-link/src/intent.rs`
(`IntentProvider`, `IntentPolicy`/`IntentChooser`/`NativeIntentDispatcher`/
`IntentActivitySink` traits) but it is completely unwired: no crate depends on
it, nothing constructs it, and the manifest format it depends on
(`["archetype", slug, protocol, ...]` tags) isn't parsed anywhere. This plan
wires all of it together, end to end, using the real published napplets'
existing wire format (confirmed against `@napplet/vite-plugin` and
`@napplet/sdk` source in `/Users/pablofernandez/Work/29napplet`) rather than
inventing a new one.

Deep-dived and confirmed during planning (current line numbers, not stale):
`crates/artifact/src/manifest.rs` tag-parsing loop, `VerifiedManifest`
accessors; `crates/runtime-ffi/src/lib.rs` `open_runtime_controller` (provider
composition, ~1285-1494), `RuntimeController` struct (~1080-1106), `install`/
`uninstall_build` FFI methods (~2201-2265 / ~2337-2356), the existing
`launch()` FFI method (~2636-2674) that derives `required_domains` from a
verified artifact; `crates/provider-inc/src/lib.rs` session/subscription
model; `crates/provider-link/src/intent.rs` in full; the Swift
`prepareInstalledArtifact`/`bringToFront`/`handleNativeAction` +
`NativeIncActionExecutor` native-push pattern in ContentView.swift and
`platforms/apple/Sources/NMPNativeRuntimeApple/`.

**Key finding that simplifies the design**: `Principal` (manifest_author +
d_tag + aggregate_hash) *is* an exact-build identity — it maps 1:1 to Swift's
`WorkbenchExactBuildIdentity`. No separate identity-resolution step is needed
between an intent handler registration and the window/catalog lookup Swift
already does.

**Key architectural decision**: rather than a new crate (as one background
investigation suggested), the `NativeIntentDispatcher` implementation lives
directly in `runtime-ffi` as a new sibling module. `runtime-ffi` already
depends on `runtime-app` and `provider-inc`; adding a `provider-link`
dependency there gives one place that legitimately holds concrete handles to
all three (`RuntimeApp`, `IncProvider`, `IntentProvider`) without a new crate's
dependency-graph overhead. This mirrors the existing `theme_provider`/
`config_provider: Option<Arc<...>>` fields already kept concrete (not
type-erased) on `RuntimeController` for the same reason.

## Scope for this pass (explicit MVP boundary)

- Default-handler dispatch only (`handler: "default"`), matching the SDK's
  `intent.open(archetype, payload, opts)` sugar — no chooser UI
  (`IntentChooser` stays `CancelIntentChoose`-equivalent but unreachable since
  policy never requests `Choose`).
- No native confirmation dialog before dispatch (`confirmation_required:
  false`) — the existing Permission Review grant for the `"intent"` domain is
  the user-consent gate; NAP-INTENT dispatch is not sensitive beyond that.
- If the handler's exact build isn't installed, or lacks a launch-permitted
  grant already, the intent fails honestly (`NativeIntentOutcome::Failed`,
  surfaced to the caller napplet as `ok:false`) rather than popping a
  permission sheet mid-flow. The user opening the handler napplet once
  normally (going through Permission Review) is what unblocks future intents.

## 1. Manifest archetype parsing (`crates/artifact`)

New file `crates/artifact/src/archetype.rs`:
- `pub struct ArchetypeDeclaration { pub slug: Arc<str>, pub protocol: Arc<str> }`
  — one entry per `["archetype", slug, protocol, ...]` tag (the trailing
  `kind:N` fields are validated for shape but not retained; nothing consumes
  them yet).
- `fn parse_archetype_tag(fields: &[String], limits: ...) -> Result<ArchetypeDeclaration, ManifestError>`
  validating: `fields.len() >= 3`, slug passes the same rule
  `provider-link`'s `valid_slug` uses (lowercase ascii/digit/`-_.`, ≤256
  bytes — duplicate the same predicate; `provider-link` isn't a dependency of
  `artifact` and shouldn't become one for this), protocol is non-empty/bounded
  and starts with `"napplet:"` (mirrors `provider-link::intent`'s
  `valid_declaration`), any fields after index 2 match `^kind:[0-9]+$`.

In `crates/artifact/src/manifest.rs`:
- Add one match arm for `"archetype"` in the tag loop (~line 233-313),
  alongside `"requires"`/`"server"`, calling into the new module. Accumulate
  into a `Vec<ArchetypeDeclaration>` bounded by a new
  `ManifestEventLimits.maximum_archetypes` (default matching
  `IntentProviderLimits::maximum_archetypes` = 128, but independently
  configurable).
- Add `archetypes: Arc<[ArchetypeDeclaration]>` field to `VerifiedManifest`
  (private, following the `requirements`/`servers` convention) and a
  `pub fn archetypes(&self) -> impl ExactSizeIterator<Item = &ArchetypeDeclaration>`
  accessor (~near line 490-496).
- Extend `KNOWN_REQUIREMENTS` handling: no change needed — `"intent"` is
  already a known `requires` domain; a napplet that declares archetypes still
  separately declares `["requires","intent"]` for the capability grant.

Test: extend the existing `signed_named_manifest` fixture pattern
(`manifest.rs:1194-1206`) with a case constructing
`vec!["archetype".into(), "nip29-group".into(), "napplet:nip29-group/open".into()]`
tags and asserting `.archetypes()` round-trips correctly, plus a rejection
case for a malformed protocol (doesn't start with `napplet:`).

## 2. `IncProvider` native push (`crates/provider-inc`)

Add to `impl IncProvider` in `crates/provider-inc/src/lib.rs` (near
`census`/`enforce_acl`, ~line 426):

```rust
pub fn native_push(
    &self,
    target: SessionId,
    topic: &str,
    sender: &str,
    payload: BoundedJson,
) -> Result<(), IncNativePushError>
```

Locks `self.state`, looks up `sessions.get(&target)`, requires
`.ready && .subscriptions.contains(topic)` (else
`IncNativePushError::NotSubscribed`), then pushes via the session's own
`outbound: ProviderPushSender` reusing `emit`'s exact envelope shape
(`{"topic","sender","payload"}` → `"inc.event"`, mirroring
`emit()` lines 618-626). New small error enum
(`Unknown`/`NotSubscribed`/`Push(ProviderPushError)`).

No readiness-polling primitive exists — the dispatcher (section 4) is
responsible for retrying `native_push` with bounded backoff until the target
subscribes or a timeout elapses, since subscription only happens after the
target napplet's own JS calls `inc.subscribe(convention, cb)` on boot.

## 3. Runtime-ffi composition wiring

`crates/runtime-ffi/Cargo.toml`: add
`nmp-native-provider-link = { path = "../provider-link" }`.

New file `crates/runtime-ffi/src/intent_dispatch.rs`:
- `pub struct RuntimeIntentDispatcher` holding: `app: OnceLock<Weak<RuntimeApp>>`,
  `intent_provider: OnceLock<Weak<IntentProvider>>`, `inc_provider:
  Arc<IncProvider>`, `artifacts: Arc<Mutex<BTreeMap<Principal,
  Arc<VerifiedArtifactHandle>>>>` (share `RuntimeController`'s own field via
  `Arc` rather than duplicating install/uninstall bookkeeping), plus a
  `handles: Mutex<BTreeMap<Arc<str>, Cancellation>>` for `cancel()`.
- `impl NativeIntentDispatcher`: `try_dispatch` upgrades both `OnceLock`s
  (returns `Unavailable` if either is unset — should never happen once
  construction finishes), mints a handle string, spawns a background
  `std::thread` (mirroring the existing push-delivery spawn pattern in
  `runtime-app`) that:
  1. Snapshots `app.sessions()` for an existing session with
     `principal == request.handler` in `Launching|Running|Suspended`; if none,
     looks up the handler's `VerifiedArtifactHandle` from `artifacts`, derives
     `required_domains` the same way `launch()` does (~runtime-ffi lib.rs
     2636-2674 — factor the domain-derivation body out to a shared free
     function both `launch()` and this call), and dispatches
     `PlatformCommand::Launch`.
  2. Emits the native focus/launch signal to Swift (section 4) so a window
     actually appears — dispatch alone never creates UI.
  3. Polls (bounded attempts + sleep backoff, small fixed cap — this thread
     is off the request-handling path) for the session to become ready, then
     calls `inc_provider.native_push(session, &request.convention.., &request.caller.d_tag(), request.payload)`.
  4. On success calls `intent_provider.complete(token, Handled{window_id:
     None})`; on any failure/timeout, `complete(token, Failed)`.
  `cancel(&self, native_handle)` marks the tracked `Cancellation` cancelled so
  the polling loop exits early.

In `open_runtime_controller` (~1285-1494):
- Create `let app_cell = Arc::new(OnceLock::new());` and
  `let intent_provider_cell = Arc::new(OnceLock::new());` before providers are
  built.
- Build `let inc_provider_concrete = Arc::new(IncProvider::with_native_actions(...))`
  (keep the concrete `Arc` instead of only the type-erased binding at
  line 1361-1383) and `let dispatcher = Arc::new(RuntimeIntentDispatcher {
  app: app_cell.clone(), intent_provider: intent_provider_cell.clone(),
  inc_provider: inc_provider_concrete.clone(), artifacts: artifacts_cell.clone() })`
  — this requires hoisting `self.artifacts`'s `Mutex<BTreeMap<...>>`
  construction (currently inline in the `RuntimeController` struct literal,
  line 1477) to before this point so both the dispatcher and the controller
  share the same `Arc<Mutex<...>>`.
- `let intent_provider = Arc::new(IntentProvider::new(Arc::new(DefaultOnlyIntentPolicy), Arc::new(CancelIntentChoice), dispatcher, Arc::new(NoopIntentActivity), IntentProviderLimits::default())?);`
  push `(intent_provider.clone() as Arc<dyn Provider>)` into `providers`.
- After `RuntimeApp::open(...)` succeeds (after line ~1455): `app_cell.set(Arc::downgrade(&app)).ok(); intent_provider_cell.set(Arc::downgrade(&intent_provider)).ok();`.
- Add `intent_provider: Arc<IntentProvider>` field to `RuntimeController`
  (concrete, mirrors `theme_provider`/`config_provider`).

New `DefaultOnlyIntentPolicy` (small, in `intent_dispatch.rs` or inline):
`allow: true, allow_specific_handler: false, confirmation_required: false,
reveal_candidates: true` for every request — the MVP boundary from the scope
section above, replacing `ConfirmEveryIntent`.

`install()` (~2201-2265): after the `InstallVerified` dispatch succeeds,
if `artifact.handle.manifest().archetypes()` is non-empty, group entries by
`slug` into `Vec<IntentHandlerDeclaration>` (`archetype: slug, actions:
{"open"}, conventions: {protocol, ...}`) and call
`self.intent_provider.register_handler(principal, declarations)`, recording a
refusal (not a hard failure of install) if it errors.

`uninstall_build()` (~2337-2356): after confirming `!remains_installed`, call
`self.intent_provider.unregister_handler(&principal)`.

## 4. Native focus/launch signal into Swift

New small UniFFI-exposed callback interface (separate from
`NativeIncActionExecutor`, which is semantically tied to an already-live
session/window and hard-refuses otherwise — see `handleNativeAction`'s
`"Refused: INC action came from an unopened exact build"` at ContentView.swift
~1328): `NativeIntentActivationExecutor` with one method,
`focusOrLaunch(handler: NativeRuntimePrincipal)`, defined in the UDL/proc-macro
alongside the existing `NativeIncActionExecutor` in `crates/runtime-ffi`, and
threaded through `RuntimeConfig`/`open_with_all_native_capabilities` the same
way `inc_action_executor` already is. `RuntimeIntentDispatcher` calls this
(if present) right after deciding a session needs to exist, before polling for
readiness.

Swift side, `platforms/apple/Sources/NMPNativeRuntimeApple/`:
- New small file mirroring `NativeIncActionRouter.swift`: a
  `MacOSIntentActivationExecutor` implementing the generated protocol,
  bouncing onto `DispatchQueue.main.async`, calling a registered
  `@Sendable (NativeRuntimePrincipal) -> Void` handler.
- `NativeRuntimeProfile.setIntentActivationHandler(_:)`, mirroring
  `setIncActionHandler` (`RuntimeNappletSession.swift:1255-1259`).

`ContentView.swift`:
- Register in the same bootstrap `.task(id:)` block that sets
  `setIncActionHandler` (~229-242): `profile.native.setIntentActivationHandler { principal in Task { @MainActor in handleIntentActivation(principal) } }`, torn down alongside the existing `setIncActionHandler(nil)` (~324).
- New `handleIntentActivation(_ principal: NativeRuntimePrincipal)`:
  builds `WorkbenchExactBuildIdentity(manifestAuthor:dTag:aggregateHash:)`
  from the principal's fields (1:1 mapping, no lookup needed — see Context).
  If a window for that identity already exists, `mutateLayout {
  $0.bringToFront(existing.id) }` + `pushFullWindowIfNeeded` (same as
  `handleNativeAction`). Otherwise resolve the installed artifact (same
  lookup `performReacquire`/`handleCatalogInstallation` already use) and call
  the existing `prepareInstalledArtifact(_:identity:deferPermissionPresentation:)`
  — reusing it as-is rather than duplicating its window-creation/permission-
  gating logic. If the handler isn't installed at all, just set `activity`
  to a refusal string (no window, no crash).

No `NSWindow`-level raise is added — RuntimeWorkbench is single-`NSWindow`
today (confirmed) and `bringToFront`/`pushFullWindowIfNeeded` already is the
established in-app equivalent used by `handleNativeAction`.

## 5. `napplet.intent` — nothing new needed in the JS client

Confirmed: `@napplet/sdk`'s `intent.open/invoke/available/handlers/onChanged`
already exists and talks to the `"intent"` NAP domain exactly as
`IntentProvider`'s `call()` dispatcher expects (`invoke`/`available`/
`handlers` actions, matching wire shapes). **No trusted-shell.js change is
needed** — the compatibility prelude in
`web/trusted-shell/trusted-shell.js`/the Apple copy already generically
projects any domain the manifest declares and the runtime negotiates (this
was confirmed and is what the earlier, reverted "domain allowlist hardening"
detour in this session established). Once `"intent"` is a registered
provider (section 3) and a napplet's manifest requires it, `Bridge::negotiate`
includes it automatically and the existing generic projection handles the
rest. This section is verification-only, not implementation.

## Verification

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features`,
  `cargo test --workspace` after each of sections 1-3 (incrementally, not
  just at the end).
- New unit tests: manifest archetype tag parsing/rejection (section 1);
  `IncProvider::native_push` happy-path + not-subscribed/unknown-session
  cases (section 2); an end-to-end `runtime-ffi` test that installs two
  synthetic napplets (a caller with `requires:["intent"]` and a handler with
  `requires:["intent"]` + archetype tags), grants both, invokes intent from
  the caller's session, and asserts the handler's session receives the
  `inc.event` push and the caller receives `intent.invoke.result` with
  `ok:true` — this is the real regression test proving the wiring is genuine,
  not hardcoded.
- `bash scripts/build-runtime-swift-xcframework.sh --universal --check-bindings`
  after the UniFFI surface changes (section 3/4), same regenerate-and-commit
  workflow used earlier this session.
- Swift package tests (`xcodebuildmcp swift-package test`) plus the macOS
  xcodebuildmcp test scheme, same as the earlier gate suite this session.
- Manual/live verification (only once the above are green, and only with
  explicit go-ahead given the earlier live-app caution in this session):
  launch the workbench app, open `nip29-groups`, click a group, confirm
  `nip29-chat` opens/focuses showing that group's messages — this is the
  actual `/goal` acceptance criterion.
- The unresolved relay-loading issue (published napplet manifest is stale
  relative to local source and doesn't declare `requires:['relay','config',
  'intent']`) is a separate, already-diagnosed blocker for the *live* manual
  verification step specifically — it needs the user to republish, or a local
  rebuild/republish the user authorizes. It does not block sections 1-5 of
  this implementation, only the final live click-through check.
