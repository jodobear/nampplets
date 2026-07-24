# Trusted Web Shell

This directory is the canonical source for the bundled WebKit trust boundary.
The platform package includes it as an immutable application resource.

The top-level document is trusted. It creates exactly one untrusted
`sandbox="allow-scripts"` iframe and maps messages using `MessageEvent.source`.
The native WebKit message handler lives in an isolated content world and is not
injected into either page-world JavaScript or the napplet iframe.

Artifact HTML is parsed into an inert document before it is mounted. The shell
places the enforced CSP first and the compatibility bootstrap second in the
parsed head, ahead of every authored executable node. It does not use string or
regular-expression HTML rewriting.

Multi-file resources resolve through a native-created
`nmp-artifact://<session>/` base URL inserted outside signed `/index.html`
bytes. The scheme handler can read only exact logical paths from the
Rust-verified artifact handle adapter. It has no filesystem path API, remote
fallback, redirect handling, content sniffing, or cross-session lookup.

`trusted-shell-policy.js` is the single reviewed allowlist for the outer and
inner CSP. Regenerate the outer document with
`node scripts/render-trusted-shell.js`; tests reject policy drift. The inner
policy allows verified scheme resources only for script, style, image, media,
and font loading while retaining `connect-src 'none'`.

`shell.ping` is an internal M1 isolation canary used to prove the mapped-frame
round trip. It is not a provider API and is not evidence of NAP provider
compatibility.

The compatibility prelude has callable projections for `shell`, `storage`,
`identity`, `inc`, `theme`, `config`, and `resource`. A projection is installed
only when that exact domain is present in the Rust-negotiated launch plan;
having JavaScript support here does not advertise a provider. NAP-RESOURCE
remains unusable until the one accepted `shell.init` advertises `resource`.
Identity, INC, theme, and config pushes are accepted only from the parent
shell. NAP-THEME exposes the pinned `get()` and automatic `onChanged()` surface.
NAP-CONFIG exposes schema registration, one-shot values, a ref-counted live
subscription, shell-owned settings presentation, schema-error fan-out, and the
readonly manifest/runtime schema accessor.

NAP-RESOURCE exposes the pinned `info()`, `bytes()`, `bytesMany()`, and
`bytesAsObjectURL()` surface. `resource.cancel` remains an internal wire action:
the prelude emits it for outstanding byte operations on teardown and when a
terminal cannot be safely projected. Standard padded base64 exists only on the
native bridge. The trusted outer shell validates and converts it to bounded
`Blob` objects before posting into the sandbox, so napplet code never receives
the transport encoding. Bulk result order and per-item failures are preserved;
malformed or oversized terminals become typed resource errors. Object URLs are
bounded and revoked explicitly or during teardown. The resource object provides
no fetch, resolver, socket, relay, or other raw network primitive.

NAP-LINK exposes only the pinned `open(url, options?)` method, where `options`
may contain the untrusted display hint `label`. It emits only `link.open` and
correlates only `link.open.result`. The shell projection bounds URL, label,
correlation, and terminal text but deliberately does not decide URL schemes,
host policy, confirmation, opener selection, or success. Those decisions
remain in Rust and the native capability executor. Only `opened` and `denied`
resolve to the pinned `{ status }` result; failed, cancelled, malformed, and
late terminals reject or are ignored. There is no invented `link.cancel`,
`window.open`, location mutation, or direct navigation capability. Like
NAP-RESOURCE, the link method remains unusable until the accepted `shell.init`
advertises `link`.

Pending correlations, local event handlers, open channels, resource object
URLs, and resource batches have fixed ceilings. Page teardown rejects pending
work, cancels outstanding resource operations, revokes object URLs, unsubscribes
INC topics, closes INC channels, and returns the config subscription.

There is no public raw-HTML artifact initializer. Production executable
construction is reserved for the forthcoming Rust FFI verified-artifact
adapter; the bundled raw fixture is an explicit internal M1 canary.

Run the focused contract tests with:

```sh
node --test tests/trusted-shell.test.js
```
