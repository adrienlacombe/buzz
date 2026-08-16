# Markets indexer (listing)
#
# INDEXER_URL is a required env — no client / localhost default.
# Do not ship http://127.0.0.1:8787. Localhost is listing-proof only;
# Adrien does not want this run locally for the product client.
#
# Set a public host when ready (expected):
#   INDEXER_URL=https://markets.bitcoinmarkets.app
#
# Listing/health on whatever host INDEXER_URL points at:
#   GET {INDEXER_URL}/api/markets
#   GET {INDEXER_URL}/health
#
# Markets is adding this service in infra/aws. Hostname stays locked even
# while DNS propagates. Placeholder ECS/ALB/Route53 modules land here
# alongside relay/paymaster.
