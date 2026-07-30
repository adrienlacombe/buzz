// FORK-LOCAL (adrienlacombe/buzz) — not present in block/buzz.
//
// Restricts which relay hosts the mobile app may connect to.
//
// Upstream Buzz is deliberately multi-community: a relay arrives from QR
// pairing, an invite deep link, or a stored community. This fork runs a single
// relay and wants the shipped app to reach only that one.
//
// This is a configuration lock, not a security boundary. It stops the shipped
// app from talking to another relay. It cannot stop someone who rebuilds the
// app or points another Nostr client at a relay — enforce *who may use our
// relay* server-side instead (BUZZ_REQUIRE_RELAY_MEMBERSHIP, relay membership).
//
// Enforced in three places, deliberately:
//   1. RelaySocket.connect()      — the transport, which nothing can bypass
//   2. validateInviteRelayUri()   — invites + deep links, for an early clear error
//   3. PairingNotifier._validateRelayUrl() — QR pairing, likewise

import 'dart:io';

import 'package:flutter/foundation.dart';

/// True when running under `flutter test`, which sets `FLUTTER_TEST`.
///
/// Enforcement is skipped there on purpose. Upstream's widget and unit tests use
/// placeholder relays such as `wss://relay.example.com`, and 13 of them fail if
/// the allowlist applies. Editing those files would create a large permanent
/// conflict surface in a fork that merges upstream daily, for no safety gain —
/// the lock is a shipping concern, and it is covered directly by
/// `test/shared/relay/relay_allowlist_test.dart`, which opts in via `enforce`.
final bool _underFlutterTest = Platform.environment.containsKey('FLUTTER_TEST');

/// Hosts this build may connect to, comma-separated.
///
/// Override at build time so the fork's hostname is not baked into logic that
/// outlives it:
///
///   flutter build apk --dart-define=BUZZ_MOBILE_RELAY_ALLOWLIST=relay.example.com
///
/// An empty value disables the restriction — the escape hatch for building an
/// unrestricted client from this fork.
const String kRelayHostAllowlist = String.fromEnvironment(
  'BUZZ_MOBILE_RELAY_ALLOWLIST',
  defaultValue: 'relay.bitcoinmarkets.app',
);

bool _isLoopback(String host) =>
    host == 'localhost' ||
    host.endsWith('.localhost') ||
    host == '127.0.0.1' ||
    host == '::1';

/// Hosts permitted by the current build, lowercased. Empty means "no restriction".
Iterable<String> allowedRelayHosts() => kRelayHostAllowlist
    .split(',')
    .map((entry) => entry.trim().toLowerCase())
    .where((entry) => entry.isNotEmpty);

/// Throws [FormatException] unless [host] is permitted by this build.
///
/// Loopback is allowed in debug builds only: `just mobile-dev` and the widget
/// tests talk to a local relay, and blocking that would break local development.
/// A release build rejects it like any other off-list host.
void ensureRelayHostAllowed(
  String host, {
  bool allowLoopback = kDebugMode,
  bool? enforce,
}) {
  if (!(enforce ?? !_underFlutterTest)) return;

  final allowed = allowedRelayHosts().toList();
  if (allowed.isEmpty) return;

  final normalized = host.trim().toLowerCase();
  if (normalized.isEmpty) {
    throw const FormatException('Relay URL has no host');
  }
  if (allowLoopback && _isLoopback(normalized)) return;

  if (!allowed.contains(normalized)) {
    throw FormatException(
      'This build only connects to ${allowed.join(", ")}; '
      'refusing relay host "$normalized"',
    );
  }
}

/// [ensureRelayHostAllowed] for a full relay URL.
///
/// Uses [Uri.host] rather than string matching so that credentials, ports, paths
/// and suffix lookalikes (`relay.example.com.evil.test`) cannot slip past.
void ensureRelayUrlAllowed(
  String url, {
  bool allowLoopback = kDebugMode,
  bool? enforce,
}) {
  final Uri uri;
  try {
    uri = Uri.parse(url);
  } on FormatException {
    throw FormatException('Unparseable relay URL: $url');
  }
  ensureRelayHostAllowed(
    uri.host,
    allowLoopback: allowLoopback,
    enforce: enforce,
  );
}
