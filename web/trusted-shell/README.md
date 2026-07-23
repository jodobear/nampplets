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

There is no public raw-HTML artifact initializer. Production executable
construction is reserved for the forthcoming Rust FFI verified-artifact
adapter; the bundled raw fixture is an explicit internal M1 canary.

Run the focused contract tests with:

```sh
node --test tests/trusted-shell.test.js
```
