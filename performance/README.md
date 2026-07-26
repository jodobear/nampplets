# Performance evidence v1

This directory defines the immutable, language-neutral input contract for
Nampplets performance evidence. It defines evidence, not acceptable product
performance. Collectors and later policy gates must emit or consume this
contract without creating another timing, refusal, comparison, or budget
truth.

## Artifacts

| Schema identifier | Purpose |
| --- | --- |
| `urn:nampplets:performance:result:v1` | One benchmark, one cold-or-warm state, and one measured run. |
| `urn:nampplets:performance:comparison:v1` | Two result references, their complete comparison identities, comparability, and explicit confidence disposition. |
| `urn:nampplets:performance:budget:v1` | Evidence and rationale required to propose or ratify a budget. It is not a verdict. |

Every object rejects unknown members. Every string and array is bounded.
Durations and resource values use explicit integer units. JSON floating-point
numbers, non-finite numbers, duplicate members, and unrecognized schema
identifiers are refused.

`fixtures/result-v1.valid.json` is a canonical diagnostic result containing
success, refusal, and deadline outcomes. No accepted baseline or budget ships
with v1.

## Authority

Raw, ordered samples and protocol metadata are the authority. For a result,
`scripts/ci/validate_performance_evidence.py` recomputes and byte-compares:

- the declared sample count and outcome counts;
- the p50, p95, p99, maximum, and exact population variance of every non-empty
  outcome distribution;
- typed refusal groups;
- upper diagnostic outliers;
- result disposition;
- the comparison key; and
- the artifact checksum.

Producer summaries are assertions, never trusted inputs. Any disagreement
refuses the entire artifact with a stable code; the validator emits no partial
comparison or budget decision.

Outcomes remain distinct:

- `success` completed the measured operation;
- `refused` is an owning runtime or provider's typed semantic refusal;
- `failed` is a typed non-refusal failure; and
- `deadline_exceeded` is the timeout outcome.

A refusal code containing `timeout` is invalid. A producer cannot relabel a
deadline as capacity or another runtime refusal.

### Percentiles and variance

Percentiles use nearest rank over the sorted integer durations:

```text
rank = ceil(percent * n / 100)
value = sorted[rank - 1]
```

Population variance is an exact, unreduced rational computed with
arbitrary-precision integer arithmetic:

```text
numerator   = n * sum(x^2) - sum(x)^2
denominator = n^2
```

The numerator and denominator are canonical unsigned decimal strings. The
denominator is positive. A singleton is therefore `0/1`. V1 does not emit a
rounded or floating-point variance.

The fixed `tukey_upper_3_iqr_v1` policy marks a success sample as a diagnostic
outlier when it is strictly greater than `Q3 + 3 * (Q3 - Q1)`, with Q1 and Q3
using the same nearest-rank rule. Outliers remain in every count and
distribution. A failed sample, deadline, or flagged outlier makes the result
`diagnostic`; typed refusals remain valid measured outcomes.

## Comparability and confidence

The comparison key is the lowercase SHA-256 digest of the canonical JSON
encoding of the complete `identity` object. It intentionally excludes source
revision and artifact locator so different revisions can be compared, while
including benchmark, cold-or-warm state, reset scopes, fixture/cardinality,
protocol, build mode/toolchain, environment, power/thermal state, and resource
measurement availability.

Comparison artifacts repeat each referenced result's complete identity so an
offline validator can recompute both keys without fetching an artifact or
using the network. Differences produce ordered typed mismatch codes.
In particular, cold and warm evidence produces `state_mismatch` and an
`incomparable` disposition.

Confidence is never implied:

- `observed_only` and `incomparable` require `not_evaluated` confidence plus a
  bounded reason;
- `method_evaluated` requires a named and revisioned method, a ratification
  locator, a bounded result, and an evidence locator.

Selection, ratification, and interpretation of a confidence method remain
owned by issue #173.

## Canonical JSON and checksums

`nampplets-canonical-json-v1` is UTF-8 JSON with:

- object members sorted by Unicode key;
- no insignificant whitespace or trailing newline;
- unescaped non-ASCII text;
- separators `,` and `:`; and
- integer numbers only.

The artifact checksum is lowercase SHA-256 over that canonical encoding after
removing the root `checksum_sha256` member. The checksum does not include
itself. A comparison key is computed the same way over the result `identity`
object. Because v1 admits no floating-point values, this profile is
deterministic across supported producer languages without numeric
normalization ambiguity.

## Safety ceilings are not budgets

The parser and work-runner safety ceilings are:

| Ceiling | Limit | Typed refusal |
| --- | ---: | --- |
| Input artifact | 16 MiB | `input_too_large` |
| Measured samples | 10,000 | `sample_limit_exceeded` |
| Warmups | 1,000 | `warmup_limit_exceeded` |
| Per-sample deadline | 5 minutes | `sample_deadline_limit_exceeded` |
| Per-run deadline | 2 hours | `run_deadline_limit_exceeded` |

They bound parser memory and work duration. They do not assert acceptable
latency, throughput, CPU, RSS, capacity, or any other product threshold.
Exceeding one refuses the whole input before summary, comparison, confidence,
or budget evaluation.

## Validator

The validator uses only the Python standard library and reads at most
16 MiB plus one byte:

```sh
python3 scripts/ci/validate_performance_evidence.py \
  performance/fixtures/result-v1.valid.json
```

It emits canonical JSON and exits zero only for a fully accepted artifact.
Focused tests run with:

```sh
python3 -m unittest scripts/ci/test_validate_performance_evidence.py
```

The Gherkin in issue #197 is contract prose for this Python parser/validator
slice. This directory does not add a duplicate Cucumber runner and does not
claim `bdd:executable`.
