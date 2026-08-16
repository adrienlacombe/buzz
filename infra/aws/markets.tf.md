# Markets indexer (listing)
#
# Product INDEXER_URL (required env, or this public host):
#   INDEXER_URL=https://markets.bitcoinmarkets.app
#
# NO localhost default. Adrien does not want this run locally.
# http://127.0.0.1:8787 was listing-proof only — do not ship it as a client default.
#
# Listing/health (no auth):
#   GET {INDEXER_URL}/api/markets
#   GET {INDEXER_URL}/health
#
# v1 market:
#   address      0x023b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8
#   title        Bitcoin difficulty after next retarget
#   marketType   lognormal
#   xAxisLabel   Difficulty
#   collateral   BTC (UI copy)
#
# Do NOT put ADMIN_API_KEY or AVNU_API_KEY in the Buzz repo / client / PR.
# Listing/health do not need ADMIN_API_KEY. AVNU_API_KEY belongs only on
# buzz-avnu-proxy at runtime.
#
# Hostname is locked even while Markets wires the service in infra/aws and
# DNS propagates.
