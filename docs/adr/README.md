# Architecture decision records

All M0 decisions are accepted as architecture contracts but the compatibility
baseline remains unratified until the signoff block in `compatibility.lock` is
complete.

| ADR | Decision |
| --- | --- |
| [0001](0001-pinned-legacy-compatibility.md) | Pin compatibility; existing napplets first; macOS first; accepted artifact modes; unsupported domains absent |
| [0002](0002-exact-build-principals-and-grants.md) | Publisher-qualified exact-build principals; no silent grant transfer |
| [0003](0003-trusted-shell-webview-boundary.md) | Native Rust runtime; coarse WebViews; trusted shell; inner sandbox; hidden native bridge |
| [0004](0004-runtime-and-nmp-storage-boundary.md) | NMP public facade; separate runtime persistence; one NMP trust profile; honest scoped evidence |
| [0005](0005-additive-surface-separation.md) | Optional surface; host bindings; renderer restrictions; preserved binding; inert descriptor; revisioned state; typed actions |
| [0006](0006-no-dynamic-native-code.md) | No dynamic native code or protocol cartridges in v1 |
| [0007](0007-pinned-good-morning-capability-profile.md) | Exact-build Rust compatibility profile for the unchanged published Good Morning artifact |

## Product-spec decision coverage

The numbered decisions in product specification section 26 map as follows:

| Spec decisions | ADR |
| --- | --- |
| 1, 2, 6, 10, 22 | 0001 |
| 20, 21 | 0002 |
| 3, 7, 8, 9 | 0003 |
| 4, 5, 19, 23, 24 | 0004 |
| 11, 12, 13, 14, 15, 16, 17, 18 | 0005 |
| 25 | 0006 |
