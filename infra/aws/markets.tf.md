# Markets indexer (listing)

# Hostname is locked for the Bitcoin Markets product:
#   https://markets.bitcoinmarkets.app
#
# Clients read INDEXER_URL (required in deploy). Production value:
#   INDEXER_URL=https://markets.bitcoinmarkets.app
#
# Endpoints:
#   GET {INDEXER_URL}/health
#   GET {INDEXER_URL}/api/markets
#
# Never default INDEXER_URL to http://127.0.0.1:8787 — loopback is
# listing-proof only. Infra wiring for the indexer service lands here;
# the hostname above is stable even while DNS propagates.

# Placeholder: ECS/service modules for the indexer will be added alongside
# the existing relay/paymaster stacks. Until then, point INDEXER_URL at the
# product host above.
