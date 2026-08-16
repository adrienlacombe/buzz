# Markets indexer (listing)
#
# Desktop default INDEXER_URL for the v1 market listing:
#   http://127.0.0.1:8787
# (Adrien's machine, same host — not sslip.io). Override via INDEXER_URL /
# VITE_INDEXER_URL for a public host when ready.
#
# Listing/health on whatever host INDEXER_URL points at:
#   GET {INDEXER_URL}/api/markets
#   GET {INDEXER_URL}/health
#
# Example listing row:
#   address    0x23b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8
#   title      Bitcoin difficulty after next retarget
#   marketType lognormal
#   xAxisLabel Difficulty
#
# Cloud / CI agents cannot reach Adrien's localhost — do not live-fetch the
# default URL as a build or test dependency.
#
# Markets may later add a public service in infra/aws; until then the desktop
# client defaults to the local indexer above.
