NIP-SW
======

Starknet Wallet Binding
-----------------------

`draft` `optional` `client`

This NIP defines a verifiable, chain-scoped link between a Nostr identity and a
Starknet account contract as `kind:30178` addressable events. The event is signed
by the Nostr key and carries a **Starknet-side attestation** — a signature
produced by the account itself over the Nostr pubkey — so the binding is proven
in both directions rather than merely claimed.

A conforming relay verifies the attestation on-chain at ingest and rejects
bindings that fail, so a stored binding is an attested one.

## Motivation

A Nostr identity and a Starknet account are both keypair-rooted, and secp256k1
Nostr keys are curve-compatible with Starknet's `secp256k1` account validation.
That makes it tempting to derive one from the other, or to publish an address in
profile metadata and treat it as the user's wallet.

Both shortcuts are unsafe:

- **Deriving the account from the Nostr identity key** makes a signing identity
  into a spending key. In Buzz the identity secret is deliberately shared with
  automation — `BUZZ_PRIVATE_KEY` is injected into agent subprocess environments
  — and the identity is disposable by design (ephemeral fallback when the OS
  keyring is unreachable, plus wipe and re-import paths). Neither property is
  compatible with custody.
- **Publishing an address as a bare claim** is spoofable. Any pubkey can assert
  any address. A client that displays or sends to an unattested address can be
  pointed at an attacker's account.

This NIP takes the third path: the account keeps its own key, Buzz never holds
it, and the link between the two identities is independently verifiable.

## Non-Goals

This NIP does not define custody, transaction construction, signing, session
keys, account deployment, or payment flows. It does not make Buzz a wallet. It
carries no private key material and no seed phrases.

It does not remove the client's obligation to re-verify before acting on an
address. Relay verification raises the floor; it cannot speak for the present.
See [Relay behavior](#relay-behavior).

## Terminology

- **binding**: the addressable coordinate `(pubkey, 30178, d)` where `d` is a
  Starknet chain id.
- **head**: the winning latest event for a binding under NIP-01 replacement.
- **attestation**: a Starknet signature over a SNIP-12 typed-data message that
  commits to the Nostr pubkey, the chain id, and a timestamp.
- **attested binding**: a head whose attestation verifies against the named
  account contract at the time of checking.
- **unattested binding**: a head whose attestation is absent, malformed, or
  fails verification. An unattested binding is a claim, not a fact. A conforming
  relay never stores one, but clients still encounter them from non-conforming
  relays and from stale local caches.

## Relationship to Other NIPs

Uses [NIP-01](01.md) addressable-event replacement semantics and
[NIP-09](09.md) deletion requests. The `i` tag follows
[NIP-39](39.md) external identity conventions, which is what makes indexed
reverse lookup possible — Buzz filters only index single-letter tags.

Deliberately **not** NIP-57 (zaps) or NIP-47 (Nostr Wallet Connect). Those move
value; this only publishes an identity link. NIP-47 remains the appropriate
mechanism if Buzz later needs to *initiate* payments, precisely because it keeps
the spending key outside the client.

## Event

`kind:30178` is addressable, keyed by `(pubkey, kind, d)`.

The `d` tag MUST be the Starknet chain id as its ASCII short-string form — for
example `SN_MAIN` or `SN_SEPOLIA`. Chain-scoping the `d` tag gives each user one
current binding per chain under NIP-01 last-write-wins, rather than collapsing
mainnet and testnet into a single slot.

Required tags:

```jsonc
[
  ["d", "SN_MAIN"],
  ["i", "starknet:SN_MAIN:0x04a5…", "0x1f3c…"]
]
```

The `i` tag carries `starknet:<chain_id>:<address>`, enabling reverse lookup
(address → npub) via `{"kinds":[30178],"#i":["starknet:SN_MAIN:0x04a5…"]}`. Its
third element MAY be the attestation message hash, for correlation without
parsing content.

Content is a JSON object:

```jsonc
{
  "address": "0x04a5…",              // account contract address, felt hex
  "chain_id": "SN_MAIN",             // MUST equal the d tag
  "class_hash": "0x02b3…",           // optional; lets clients infer signer scheme
  "signer_scheme": "secp256k1-ecdsa", // "stark" | "secp256k1-ecdsa" | "secp256r1"
  "attestation": {
    "scheme": "snip12",
    "signature": ["0x…", "0x…"],
    "signed_at": 1785400000
  }
}
```

`address` MUST match the address in the `i` tag. `chain_id` MUST match the `d`
tag. A client encountering a mismatch MUST treat the binding as unattested.

## Attestation

The payload deliberately carries **no message hash**. Verifiers derive it; they
never accept it from the submitter.

This is not a hardening detail — accepting a submitted hash makes the scheme
forgeable with public data alone. Starknet transaction signatures are on-chain,
so an attacker can lift any `(tx_hash, signature)` pair from a victim's account
and publish it as an attestation under their own Nostr identity. The account
confirms the signature is valid, because it is: over a message that says nothing
about the binding. Removing the field removes what there was to spoof.

The attested message is SNIP-12 typed data whose fields commit to, at minimum:

- the Nostr pubkey (32-byte hex, as published)
- the chain id
- `signed_at`
- a domain separator naming this NIP

Committing to the chain id prevents replaying a mainnet attestation onto a
testnet binding. Committing to the Nostr pubkey is what makes the binding
directional — a signature over the address alone would be replayable by anyone
who observed it.

Verification is a `is_valid_signature(derived_hash, signature)` call on the
account contract per SNIP-6, at entry point
`0x028420862938116cb3bbdbedee07451ccc54d4e9412dbef71142ad1980a30941`
(`starknet_keccak("is_valid_signature")`). Because it is the *account* that
validates, this works uniformly across signer schemes: Stark curve, secp256k1,
secp256r1, multisig, or any custom `__validate__` — the verifier does not need to
know which.

A return of the `VALID` short string (`0x56414c4944`) means valid. Cairo 0-era
accounts return `TRUE` (`0x1`) instead and both remain deployed, so accept
either; anything else, including `0`, an empty return, or a revert, is a
rejection.

Both signer and verifier build the SNIP-12 document from the same source (see
`buzz_core::snip12::BindingMessage::typed_data_json`) and hash it with the same
SNIP-12 implementation, so compatibility does not rest on either side
hand-rolling type strings.

## Client behavior

On a conforming relay a stored binding was attested at ingest, so a client need
not verify merely to distinguish a claim from a fact.

A client MUST nonetheless re-verify before any action whose safety depends on the
address — sending value, or displaying it as a payment destination. Ingest-time
attestation is a statement about the past (see
[Security](#security-considerations)).

A client SHOULD:

- treat a binding from an unknown or non-conforming relay as unattested until it
  verifies it itself;
- display attested and unattested bindings differently, and never silently fall
  back to an unattested one;
- treat a missing binding as "no wallet", never as an error state.

Queries MUST include explicit `kinds`. A kindless filter hits the relay's
p-gate and returns 403.

## Relay behavior

A conforming relay MUST verify the attestation before storing the event, and MUST
reject a kind:30178 event whose attestation is absent, malformed, or invalid.
Verification is a `is_valid_signature` call against the account contract named in
the payload, on the chain named by the `d` tag.

This must happen in the **ingest path**, not in `handle_side_effects()` — side
effects run after the event is stored, which is too late to reject. See
`crates/buzz-relay/src/handlers/ingest.rs`.

The relay applies NIP-01 replacement as normal once verification passes.

### Configuration

A relay MUST have an RPC endpoint configured for every chain it accepts bindings
on. A relay that accepts `SN_MAIN` and `SN_SEPOLIA` needs both. A relay with no
endpoint for a chain MUST reject bindings for that chain rather than storing them
unverified.

Because the relay is community-scoped by host, endpoint configuration is
per-deployment, not per-event. The submitting client cannot name the RPC to use —
that would let an attacker point verification at an endpoint they control.

### Failure policy

Verification MUST **fail closed**: if the RPC is unreachable, times out, or errors,
the relay rejects the event. This couples binding-write availability to RPC
availability, which is a real cost — but failing open silently breaks the single
invariant that relay-side verification exists to establish. A relay that
sometimes stores unverified bindings gives consumers a guarantee they cannot rely
on, which is worse than no guarantee at all. A deployment unwilling to accept
fail-closed availability should not verify at the relay, and should instead
document that its clients carry full responsibility.

Verification SHOULD be performed against a finalised state (accepted on L2 or
better), not pending state.

### What relay verification does and does not buy

It removes the spoofing vector: on a conforming relay, a stored binding was
attested by the named account. Clients no longer need an RPC endpoint merely to
distinguish a claim from a fact.

It does **not** make a stored binding presently true. See
[Security](#security-considerations) — this is a time-of-check/time-of-use gap,
not an implementation gap, and no amount of ingest-time rigour closes it.

Kind:30178 MUST be excluded from full-text search by adding it to the
`search_tsv` `CASE WHEN kind IN (…)` exclusion in the schema migration. Wallet
addresses have no place in message search results.

## Security considerations

**An attestation proves control at signing time, not now — and relay verification
does not change this.** Starknet accounts are upgradeable and their owners
rotatable. A binding the relay verified at ingest may, by the time it is read,
point at an account the user no longer controls. Ingest-time verification is
strictly a time-of-check/time-of-use window, and it is the sharpest limitation of
the scheme.

Clients MUST NOT paper over it: prefer recent attestations, surface `signed_at`,
and re-verify before value-bearing actions rather than trusting either a cached
result or the relay's ingest check. Deployments wanting a hard bound SHOULD adopt
a maximum attestation age policy.

**Relay verification is a trust delegation.** A client trusting "stored implies
attested" is trusting that relay's operator, its RPC endpoint, and its
correctness. That is reasonable for a community's own relay and unreasonable for
an arbitrary one — hence the client requirement above to treat bindings from
unknown relays as unattested.

**Never reuse the Nostr identity key as the account signer.** Beyond the general
anti-pattern of coupling a high-volume public signing oracle to custody, in this
codebase that key reaches agent subprocesses via `BUZZ_PRIVATE_KEY`. Any account
it controls is controllable by every agent subprocess, and by anything able to
read that process environment. If a Nostr key must have *any* on-chain authority,
it belongs behind a scoped session signer with policy limits — never as account
owner.

**Nostr signs BIP-340 Schnorr; secp256k1 account validation is ECDSA.** The key
is compatible but the schemes are not interchangeable. An implementation reusing
one key across both signature schemes runs two independent signing stacks over
the same secret, and nonce-handling mistakes there are a classic key-disclosure
route. This NIP avoids the question entirely by not reusing the key.

**Revocation** is a NIP-09 deletion request, or a replacement event with no
attestation. Clients MUST honour deletion and MUST NOT treat a previously cached
attested binding as still valid after replacement.

## Open questions

- Whether to require a maximum attestation age normatively rather than as a
  deployment policy.
- Whether `class_hash` should be required, so clients can warn on account
  implementations they do not recognise.
- Whether a companion kind should express *intent to receive* (opt-in to being
  sent value) separately from *proof of control*. They are different claims and
  conflating them may be a mistake.
- Whether the relay should advertise chains it verifies via NIP-11, so clients
  can tell "this relay verifies `SN_MAIN`" from "this relay stored something it
  could not check". Without it, "conforming relay" is not machine-discoverable
  and the client's trust decision stays manual.
- Whether re-verification should be pushed rather than polled — a relay already
  watching an RPC could detect owner rotation on a bound account and emit a
  signal, rather than every client polling independently.
