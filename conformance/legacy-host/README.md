# Legacy compatibility runner

This directory owns executable evidence for the M2 legacy compatibility gate.
It does not turn partial coverage, preflight refusal, or missing toolchain bytes
into a pass.

The runner executes every committed reference and published fixture without
modifying its bytes. It loads the repository's canonical trusted shell in a real
Chromium process, mounts the fixture under the same `sandbox="allow-scripts"`
boundary, and feeds the observations to the exact pinned
`@napplet/conformance` engine. Package archives are accepted only when their
SHA-256 matches `compatibility.lock`.

NAP-SHELL is deliberately separate:

- `shell.ready` and `shell.init` come from the pinned registry document;
- they are not package-active `@napplet/conformance` domains;
- the bounded host answers the first `shell.ready` with one scoped
  `shell.init` and verifies synchronous `supports()` behavior;
- `shell.ping` remains only an internal isolation canary and is never counted
  as registry-handshake evidence.

The same separation applies to explicit host-boundary probes. The pinned
package validator receives only envelopes whose domain is in the package's
active-domain inventory. Registry-only `shell.*`, the deliberate
`future-domain.*` forward-compatibility probe, and `conformance.*` observation
markers remain in the report and are judged by host assertions; they are not
misrepresented as package NAP traffic.

Run the committed fixtures:

```sh
python3 conformance/legacy-host/run.py \
  --allow-package-download \
  --package-cache /tmp/nampplets-npm-cache
```

When Playwright is installed in a disposable or cached `node_modules` rather
than globally, pass its parent directory with
`--playwright-module-root /path/to/node_modules`. The runner verifies that the
module is present before starting the bounded browser process.

Omit `--allow-package-download` for an offline-only run. Missing verified
archives then produce a runner prerequisite failure instead of a green report.
The browser process, fixture size, captured envelopes, and subprocess output
are all bounded.

Chromium cannot register the native `nmp-artifact:` URL scheme. The runner still
mounts the unchanged external-assets fixture and records the attempted sealed
scheme load, but reports that asset execution as `not-run`; only the Apple
package's real `WKURLSchemeHandler` integration test can turn that lane green.
The pinned package engine's boot check does not prove external asset execution,
so its verdict is shown separately.

Run the exact Kehto source corpus from an existing checkout without mutating it:

```sh
python3 conformance/legacy-host/run_kehto.py \
  --source /path/to/kehto-web
```

The runner exports the pinned commit into a temporary directory, uses exact
`pnpm@10.8.0`, installs only the 15 playground napplet projects, and performs
only a frozen offline install. To supply a content-addressed store that was
populated from this exact lockfile, add:

```sh
--dependency-store /path/to/pnpm-store
```

For example, an explicitly networked preparation step in a disposable checkout
can populate that store without weakening the later execution run:

```sh
corepack pnpm@10.8.0 install \
  --filter './apps/playground/napplets/**' \
  --frozen-lockfile \
  --ignore-scripts \
  --store-dir /path/to/pnpm-store
python3 conformance/legacy-host/run_kehto.py \
  --source /path/to/kehto-web \
  --dependency-store /path/to/pnpm-store
```

The second command still performs an offline frozen install from an exact
source archive. A missing store tarball is recorded once and projected as a
machine-readable `not-run` reason for every affected application. To clone the
exact commit into a temporary directory first, replace `--source` with
`--network`; dependency installation still remains offline.

Reports are written to:

- `conformance/reports/legacy-host.json`;
- `conformance/reports/kehto-corpus.json`.

`status: incomplete` is intentional until unchanged reference, published, and
Kehto artifacts all boot through the native host and every applicable pinned
conformance check is green.

## CI contract

CI regenerates both reports into the runner's temporary directory and requires
each runner to exit `2`, its explicit known-incomplete result. Exit `0` would
mean the milestone's full pass conditions were met and is refused by this gate
until the compatibility change deliberately updates the expectation. Exit `1`
is always a runner or prerequisite failure.

The regenerated reports are then checked by
`scripts/ci/validate_legacy_reports.py`. The validator binds their source
commits, package digests, trusted-shell bytes, fixture/application coverage,
summary arithmetic, and non-green claims back to `compatibility.lock` and the
committed corpus indexes. CI uploads these reports as evidence; it does not
overwrite the committed reports or reinterpret `not-run` as pass.
