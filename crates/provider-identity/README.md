# Identity provider

This crate implements the exact flat-envelope identity contract exported by
`@napplet/nap` 0.28.0. The npm tarball used as the executable contract is:

```text
@napplet/nap@0.28.0
sha256 ff51a33cd35e06b5067b09407fb3e381c6bfe4ef229ce8c082b3beb156ebd5b6
```

The provider covers every pinned request:

```text
identity.getPublicKey
identity.getRelays
identity.getProfile
identity.getFollows
identity.getList
identity.getZaps
identity.getMutes
identity.getBlocked
identity.getBadges
```

It also treats `identity.changed` as mandatory provider behavior. Account
observation is installed atomically with its initial snapshot, pushes have no
correlation id, sign-out pushes an empty `pubkey`, and a bounded push refusal
terminates the affected session instead of silently losing the transition.

The sole NMP engine owner implements the provider's read/observation port using
the supported `nmp::Engine` facade. Public key, kind-0 profile, kind-3 follows,
and kind-10002 NIP-65 relay metadata are supported. Operations that cannot yet
be composed honestly from the pinned facade return their typed default plus an
error; they are never fabricated.

Mapped provider open/ready/close/revoke lifecycle and the bounded, conflating
outbound push channel are consumed directly from `nmp-native-nap-bridge`. The
data port exposes public identity values and scoped evidence only. Secret keys,
signer capabilities, raw signer objects, and NMP mechanism types are not part
of this crate's API.
