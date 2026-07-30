# Two secrets, split by who is allowed to know the value.
#
# 1. runtime  — infrastructure credentials Terraform necessarily already knows
#               (it created them). Storing them here changes nothing about their
#               exposure and lets ECS inject them without env-var plaintext in
#               the task definition.
#
# 2. identity — the relay's Nostr private key. Terraform must NOT generate or
#               hold this. Every attribute Terraform manages is written to state
#               in plaintext, so a key generated here would sit unencrypted in
#               S3 and in every local plan file. You generate it out-of-band and
#               push the value with one CLI call; Terraform only creates the
#               empty container and then ignores the contents forever.

resource "aws_secretsmanager_secret" "runtime" {
  name        = "${local.name}/runtime"
  description = "Buzz relay runtime credentials (DB, S3, git hook HMAC)"

  # Dev convenience: allow immediate reuse of the name after a destroy. Raise to
  # 7-30 at the production tier so a mistaken destroy is recoverable.
  recovery_window_in_days = 0

  tags = { Name = "${local.name}-runtime" }
}

# HMAC shared secret for the relay's git policy hooks — a symmetric infra secret,
# not identity key material.
resource "random_password" "git_hook_hmac" {
  length  = 48
  special = false
}

resource "aws_secretsmanager_secret_version" "runtime" {
  secret_id = aws_secretsmanager_secret.runtime.id

  secret_string = jsonencode({
    # Assembled here rather than in the task definition so the password never
    # appears as a plaintext ECS environment variable.
    DATABASE_URL = format(
      "postgres://%s:%s@%s:%d/%s",
      aws_db_instance.main.username,
      random_password.db.result,
      aws_db_instance.main.address,
      aws_db_instance.main.port,
      aws_db_instance.main.db_name,
    )
    BUZZ_S3_ACCESS_KEY        = aws_iam_access_key.relay_s3.id
    BUZZ_S3_SECRET_KEY        = aws_iam_access_key.relay_s3.secret
    BUZZ_GIT_HOOK_HMAC_SECRET = random_password.git_hook_hmac.result
  })
}

# ── Relay identity ───────────────────────────────────────────────────────────

resource "aws_secretsmanager_secret" "identity" {
  name        = "${local.name}/relay-identity"
  description = "BUZZ_RELAY_PRIVATE_KEY — set out-of-band, never managed by Terraform"

  recovery_window_in_days = 0

  tags = { Name = "${local.name}-relay-identity" }
}

# There is deliberately NO aws_secretsmanager_secret_version for this secret.
#
# Terraform reads a managed secret version back on every refresh, which means
# managing one here would force the CI deploy role to hold
# secretsmanager:GetSecretValue on the relay's identity key. Leaving the version
# unmanaged lets oidc.tf Deny that action outright, so a compromised workflow
# cannot read the relay's private key. It also removes any path by which an
# apply could overwrite a key you set by hand.
#
# The cost is that the ECS task cannot start until you create the first version.
# That is the same practical state as a placeholder (the relay rejects a
# non-key either way) but fails with a clearer message.
#
#   aws secretsmanager put-secret-value \
#     --profile alc-tf --region eu-west-3 \
#     --secret-id "buzz-dev/relay-identity" \
#     --secret-string "$(openssl rand -hex 32)"
