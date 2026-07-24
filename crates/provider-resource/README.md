# NAP-RESOURCE provider

This crate owns the Rust policy state machine for the pinned
`napplet-web@0.28.0` resource domain. It is not a general network client and it
must not be registered until the host supplies both required capability ports:

- `ResourceNetwork`: raw DNS and HTTPS execution that connects only to the
  Rust-approved IP set, keeps the original host for TLS/SNI, follows no
  redirects, bounds response reads, and honors cancellation/deadlines.
- `SvgRasterizer`: no-network sandboxed SVG rasterization with bounded input,
  output dimensions, bytes, time, and cancellation.

Rust owns URL/scheme validation, DNS and redirect admission, private-address
refusal, MIME byte sniffing, SVG result validation, Blossom SHA-256 checks,
per-build rate/concurrency limits, request correlation, activity facts, and
lifecycle cancellation.

The JSON native bridge represents the NAP byte-string `blob` field as standard
padded base64. The trusted web projection must convert that field into a
`Blob`, revoke any object URL it creates, drop late terminals for cancelled
request IDs, and signal the runtime operation terminal. The sandbox never sees
base64, raw network authority, resolver results, IP addresses, upstream
`Content-Type`, or an unrestricted URL loader.

`data:`, `https:`, and canonical `blossom:sha256:<hex>` are the only advertised
schemes. `http:`, `htree:`, `nostr:`, unknown schemes, credential-bearing URLs,
non-public resolved addresses, and raw SVG delivery fail closed.
