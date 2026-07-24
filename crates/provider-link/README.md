# Native link and intent providers

This crate owns the Rust policy kernels for the pinned
`napplet-web@0.28.0` `link` and `intent` domains.

- `LinkProvider` accepts only absolute public `http`/`https` URLs, rejects
  credentials and local/private hosts, applies injected product policy, and
  always marks the native open as confirmation-required. It forwards the
  pinned optional `options.label` as a bounded, untrusted display hint to the
  native confirmation surface; the label is never URL or policy input.
- NAP-LINK terminals preserve the pinned public status set exactly:
  `opened` means the native handoff succeeded and a user cancellation becomes
  `denied`/`user-denied`. Native execution failures reject the pending shim
  request through an error terminal without manufacturing `failed` or
  `cancelled` public statuses.
- `IntentProvider` maintains a finite trusted registry of verified
  exact-build handlers and user-owned defaults. It validates archetypes,
  actions, conventions, behavior, payload size, explicit targeting, and every
  raw open-with choice before dispatch.
- Native implementations receive exact Rust-authorized requests and raw
  cancellation signals. They may present confirmation/choice UI and execute
  the OS operation, but they do not own routing or fallback policy.
- Outstanding work is finite, charged by the bridge lease, and synchronously
  cancelled on stop, crash, revocation, failed open, or runtime shutdown.

The crate does not open URLs or windows in tests. Fake native executors prove
the request, refusal, completion, push-delivery, and teardown contracts without
ambient external actions.
