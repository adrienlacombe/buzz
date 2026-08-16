# Markets indexer (listing)
#
# Markets is adding this service in infra/aws. Hostname is locked even if DNS
# is still propagating:
#
#   https://markets.bitcoinmarkets.app
#   (domain: bitcoinmarkets.app)
#
# Clients use required env INDEXER_URL — no localhost default. Production value:
#
#   INDEXER_URL=https://markets.bitcoinmarkets.app
#
# Listing:
#   GET {INDEXER_URL}/api/markets
#   GET {INDEXER_URL}/health
#
# Do not ship http://127.0.0.1:8787 — loopback is listing-proof only.
#
# Placeholder: ECS/ALB/Route53 modules for the indexer land here alongside the
# existing relay/paymaster stacks. Until Terraform creates the record, clients
# still point INDEXER_URL at the locked product host above.
