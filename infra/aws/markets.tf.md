# Markets indexer (listing) — desktop client only
#
# CEO-confirmed. No public hostname yet. Desktop default:
#   INDEXER_URL=http://127.0.0.1:8787
# (Adrien's shared machine localhost.) Make INDEXER_URL / VITE_INDEXER_URL
# configurable so the host can be swapped later.
#
# Unauthenticated:
#   GET {INDEXER_URL}/api/markets
#   GET {INDEXER_URL}/health
#
# v1 market:
#   address      0x023b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8
#   title        Bitcoin difficulty after next retarget
#   marketType   lognormal
#   xAxisLabel   Difficulty
#   collateral   BTC (UI copy only)
#
# ADMIN_API_KEY exists ONLY on the indexer host. Do NOT read it. Do NOT put it
# in the Buzz repo, desktop client, or PR. Listing/health do not need it.
#
# Cloud VMs cannot reach Adrien's localhost — do not live-fetch this URL from
# CI/agent. Wire the desktop client only.
