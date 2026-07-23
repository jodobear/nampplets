# ADR 0002: Bind principals and grants to exact publisher code

- Status: Accepted architecture; baseline signoff pending
- Date: 2026-07-24
- Invariants: I-06, I-07
- Requirements: FR-R03, FR-R04, FR-P01 through FR-P06

## Context

NIP-5D protocol identity is `(dTag, aggregateHash)`. Permission and storage
isolation also need publisher identity: two publishers can select the same
`dTag`, and a publisher can ship a malicious signed update.

## Decision

The runtime principal is:

```text
(manifestAuthor, dTag, aggregateHash)
```

The tuple is computed from the verified signed manifest and verified bytes
before execution. The napplet cannot submit or override any field.

Grant states are denied, ask every time, session, exact build, or managed host
policy. Sensitive grants never transfer to a new aggregate hash implicitly.
Update UI may offer an explicit reviewed carry decision. Component storage is
exact-build scoped; migration is a separately declared, bounded, user-approved
contract.

Protocol-visible NIP-5D identity is not changed.

## Consequences

- Same-name artifacts from different publishers are isolated.
- Every executable change receives a fresh sensitive permission boundary.
- Updates may require reapproval and explicit data migration.
- Activity and refusal facts remain attributable to one exact build.

## Verification

- Build B cannot read build A storage.
- Build B receives no sensitive grant after replacing A.
- Caller-supplied principal/session fields are ignored.
- Revocation cancels active non-durable work and denies later requests.
