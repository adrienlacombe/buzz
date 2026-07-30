// FORK-LOCAL (adrienlacombe/buzz) — not present in block/buzz.
//
// Covers the single-relay host allowlist. Hosts are derived from
// allowedRelayHosts() rather than hardcoded, so overriding
// BUZZ_MOBILE_RELAY_ALLOWLIST at build time does not fail the suite.

import 'package:flutter_test/flutter_test.dart';

import 'package:buzz/shared/relay/relay_allowlist.dart';

String get _allowedHost {
  final hosts = allowedRelayHosts().toList();
  expect(hosts, isNotEmpty, reason: 'test build must configure a host');
  return hosts.first;
}

void main() {
  group('ensureRelayHostAllowed', () {
    test('accepts a configured host, case-insensitively', () {
      expect(
        () => ensureRelayHostAllowed(_allowedHost, enforce: true),
        returnsNormally,
      );
      expect(
        () => ensureRelayHostAllowed(_allowedHost.toUpperCase(), enforce: true),
        returnsNormally,
      );
      expect(
        () => ensureRelayHostAllowed('  $_allowedHost  ', enforce: true),
        returnsNormally,
      );
    });

    test('rejects an unrelated host', () {
      expect(
        () => ensureRelayHostAllowed('relay.damus.io', enforce: true),
        throwsA(isA<FormatException>()),
      );
    });

    test('rejects a suffix lookalike', () {
      // The classic bypass: allowed host as a prefix of an attacker domain.
      expect(
        () =>
            ensureRelayHostAllowed('$_allowedHost.evil.example', enforce: true),
        throwsA(isA<FormatException>()),
      );
    });

    test('rejects an empty host', () {
      expect(
        () => ensureRelayHostAllowed('', enforce: true),
        throwsA(isA<FormatException>()),
      );
    });

    test('rejects loopback when loopback is not allowed', () {
      for (final host in ['localhost', '127.0.0.1', '::1', 'a.localhost']) {
        expect(
          () =>
              ensureRelayHostAllowed(host, allowLoopback: false, enforce: true),
          throwsA(isA<FormatException>()),
          reason: '$host must be rejected in a release build',
        );
      }
    });

    test('accepts loopback when loopback is allowed', () {
      // Local development and the widget tests rely on this.
      for (final host in ['localhost', '127.0.0.1', '::1']) {
        expect(
          () =>
              ensureRelayHostAllowed(host, allowLoopback: true, enforce: true),
          returnsNormally,
          reason: '$host must work for local dev',
        );
      }
    });
  });

  group('ensureRelayUrlAllowed', () {
    test('accepts configured host with scheme, port and path', () {
      for (final url in [
        'wss://$_allowedHost',
        'wss://$_allowedHost/',
        'wss://$_allowedHost:443',
        'https://$_allowedHost/media',
      ]) {
        expect(
          () => ensureRelayUrlAllowed(url, enforce: true),
          returnsNormally,
          reason: url,
        );
      }
    });

    test('is not fooled by the allowed host outside the authority', () {
      for (final url in [
        // Allowed host only in the path.
        'wss://evil.example/$_allowedHost',
        // Allowed host only in userinfo.
        'wss://$_allowedHost@relay.damus.io',
        // Allowed host only in a query parameter.
        'wss://evil.example/?relay=$_allowedHost',
        // Allowed host only in a fragment.
        'wss://evil.example/#$_allowedHost',
      ]) {
        expect(
          () => ensureRelayUrlAllowed(url, allowLoopback: false, enforce: true),
          throwsA(isA<FormatException>()),
          reason: url,
        );
      }
    });

    test('rejects a URL with no host', () {
      expect(
        () => ensureRelayUrlAllowed(
          'wss://',
          allowLoopback: false,
          enforce: true,
        ),
        throwsA(isA<FormatException>()),
      );
    });
  });
}
