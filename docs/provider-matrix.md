# Provider matrix

The M0 contract advertises no runtime provider. A domain becomes supported on a
platform only after its implementation passes the pinned package contract and
adversarial tests. Placeholder providers are forbidden.

| Domain | Pinned protocol source | macOS M0 | iOS M0 | Android M0 | Intended owner |
| --- | --- | --- | --- | --- | --- |
| `relay` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | NMP adapter |
| `identity` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | native account + NMP reads |
| `storage` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | runtime store |
| `inc` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | runtime action routing |
| `theme` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | native host |
| `keys` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | native host |
| `media` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | native media broker |
| `notify` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | native host |
| `config` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | runtime store |
| `resource` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | resource broker |
| `cvm` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | opt-in external provider |
| `outbox` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | NMP adapter |
| `upload` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | resource broker |
| `intent` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | native action routing |
| `ble` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | platform device provider |
| `webrtc` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | platform provider |
| `link` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | native host |
| `count` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | NMP adapter |
| `lists` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | NMP protocol resource |
| `serial` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | platform device provider |
| `common` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | NMP modules + host policy |
| `dm` | napplet-web 0.28.0 | unavailable | unavailable | unavailable | NMP modules + host policy |
| NAP-SHELL handshake | registry commit `6461e4b37c29` | tracked, not advertised | tracked, not advertised | tracked, not advertised | compatibility adapter |

Android additionally remains blocked on a qualified NMP Android AAR/runtime
surface. Desktop-JVM Kotlin tests are not Android evidence.
