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

The relay stores and serves bindings. It does not verify them and makes no
assertion about their validity.

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

It also does not define on-chain verification *by the relay*. See
[Relay behavior](#relay-behavior).

## Terminology

- **binding**: the addressable coordinate `(pubkey, 30178, d)` where `d` is a
  Starknet chain id.
- **head**: the winning latest event for a binding under NIP-01 replacement.
- **attestation**: a Starknet signature over a SNIP-12 typed-data message that
  commits to the Nostr pubkey, the chain id, and a timestamp.
- **attested binding**: a head whose attestation verifies against the named
  account contract at the time of checking.
- **unattested binding**: a head whose attestation is absent, malformed, or
  fails verification. An unattested binding is a claim, not a fact.

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
    "message_hash": "0x1f3c…",
    "signature": ["0x…", "0x…"],
    "signed_at": 1785400000
  }
}
```

`address` MUST match the address in the `i` tag. `chain_id` MUST match the `d`
tag. A client encountering a mismatch MUST treat the binding as unattested.

## Attestation

The attested message is SNIP-12 typed data whose fields commit to, at minimum:

- the Nostr pubkey (32-byte hex, as published)
- the chain id
- `signed_at`
- a domain separator naming this NIP

Committing to the chain id prevents replaying a mainnet attestation onto a
testnet binding. Committing to the Nostr pubkey is what makes the binding
directional — a signature over the address alone would be replayable by anyone
who observed it.

Verification is a `is_valid_signature(message_hash, signature)` call on the
account contract per SNIP-6. Because it is the *account* that validates, this
works uniformly across signer schemes: Stark curve, secp256k1, secp256r1,
multisig, or any custom `__validate__` — the client does not need to know which.

## Client behavior

A client MUST NOT present a binding as the user's wallet unless it has verified
the attestation against the named account contract.

A client SHOULD:

- re-verify before any action whose safety depends on the address, rather than
  trusting a cached result (see [Security](#security-considerations));
- display attested and unattested bindings differently, and never silently fall
  back to an unattested one;
- treat a missing binding as "no wallet", never as an error state.

Queries MUST include explicit `kinds`. A kindless filter hits the relay's
p-gate and returns 403.

## Relay behavior

The relay stores kind:30178 like any other addressable event and applies NIP-01
replacement. It **does not** verify attestations: doing so requires Starknet RPC
access, which would add an outbound network dependency and a trust surface to the
relay for no gain the client cannot achieve itself.

Consequently, **storage is not endorsement**. Relay acceptance says nothing about
whether a binding is attested.

Kind:30178 MUST be excluded from full-text search by adding it to the
`search_tsv` `CASE WHEN kind IN (…)` exclusion in the schema migration. Wallet
addresses have no place in message search results.

## Security considerations

**An attestation proves control at signing time, not now.** Starknet accounts are
upgradeable and their owners rotatable. A binding attested last year may point at
an account the user no longer controls. This is the sharpest limitation of the
scheme and clients MUST NOT paper over it: prefer recent attestations, surface
`signed_at`, and re-verify rather than caching indefinitely. Deployments wanting
a hard bound SHOULD adopt a maximum attestation age policy.

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
