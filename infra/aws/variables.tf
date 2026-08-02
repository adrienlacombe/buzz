variable "aws_region" {
  description = "AWS region to deploy into."
  type        = string
  default     = "eu-west-3"
}

variable "aws_profile" {
  description = <<-EOT
    Local AWS CLI profile used for apply. The scoped IAM user, not root.

    Must be "" in CI: GitHub Actions authenticates via OIDC and gets credentials
    from the environment, and naming a profile that does not exist there makes
    the provider fail before it tries the environment chain.
  EOT
  type        = string
  default     = "alc-tf"
}

variable "project_name" {
  description = "Short name prefixed onto every resource."
  type        = string
  default     = "buzz"
}

variable "environment" {
  description = "Environment name, part of the resource name prefix."
  type        = string
  default     = "dev"
}

# ── Networking ───────────────────────────────────────────────────────────────

variable "vpc_cidr" {
  description = "CIDR for the VPC. /16 leaves room for both subnet tiers."
  type        = string
  default     = "10.20.0.0/16"
}

# ── DNS / TLS ────────────────────────────────────────────────────────────────

variable "domain_name" {
  description = <<-EOT
    Apex domain you registered in Route 53 (e.g. "example.com"), or "" to skip
    DNS/TLS entirely and serve plain HTTP on the ALB's own DNS name.

    When set, the hosted zone must already exist — register the domain in the
    Route 53 console first so the WHOIS contact PII never enters Terraform state.
  EOT
  type        = string
  default     = ""
}

variable "relay_subdomain" {
  description = "Subdomain the relay answers on. Empty string uses the apex."
  type        = string
  default     = "relay"
}

# ── Relay container ──────────────────────────────────────────────────────────

variable "relay_image" {
  description = <<-EOT
    Relay container image. Public GHCR image needs no pull secret.

    REQUIRED — deliberately no default, so `terraform apply` fails closed rather
    than silently changing which build is running.

    CD owns this value: deploy-aws.yml passes the commit's immutable
    ghcr.io/<owner>/buzz:sha-<7> tag. With a default here, any local apply for an
    unrelated reason (flipping a flag, editing a size) also rewrote the image
    field — reverting whatever CD had deployed to a mutable :main tag, with no
    indication that a deploy had just been undone. Two writers with different
    views of one field.

    A local apply must therefore say which image it means. To keep what is
    currently deployed:

      terraform apply -var-file=dev.tfvars -var relay_image="$(
        aws ecs describe-task-definition --task-definition buzz-dev-relay \
          --profile alc-tf --region eu-west-3 \
          --query 'taskDefinition.containerDefinitions[0].image' --output text)"
  EOT
  type        = string

  validation {
    # A mutable tag defeats the point of recording what is deployed. Warn-by-error
    # on :main / :latest so a local apply cannot quietly un-pin the deployment.
    condition     = !can(regex(":(main|latest)$", var.relay_image))
    error_message = "relay_image must not use a mutable tag (:main, :latest) — pass an immutable ghcr.io/<owner>/buzz:sha-<7> tag so the running build is recorded. CD does this automatically."
  }
}

variable "relay_cpu" {
  description = "Fargate task CPU units (512 = 0.5 vCPU)."
  type        = number
  default     = 512
}

variable "relay_memory" {
  description = "Fargate task memory in MiB. Must be a valid pairing with relay_cpu."
  type        = number
  default     = 1024
}

variable "relay_desired_count" {
  description = <<-EOT
    Number of relay tasks. Keep at 1 unless you have verified the relay
    tolerates concurrent replicas — the EFS git path is shared mutable state.
  EOT
  type        = number
  default     = 1
}

variable "owner_pubkey" {
  description = "RELAY_OWNER_PUBKEY — hex Nostr pubkey granted relay-owner rights. Public value, safe in tfvars."
  type        = string
  default     = ""

  validation {
    # The relay warns and ignores a malformed value (config.rs:580), which then
    # surfaces as a confusing startup failure when membership is required. Catch
    # it at plan time instead. npub-encoded keys must be converted to hex first.
    condition     = var.owner_pubkey == "" || can(regex("^[0-9a-f]{64}$", var.owner_pubkey))
    error_message = "owner_pubkey must be 64 lowercase hex characters (a 32-byte x-only pubkey), or \"\". Convert an npub to hex first."
  }
}

variable "serve_git_web_gui" {
  description = <<-EOT
    BUZZ_SERVE_GIT_WEB_GUI. Serves the browser repo UI at /repos for the git
    repositories the relay hosts. Defaults to false in buzz-relay.

    The web assets already ship in the image (the Dockerfile sets
    BUZZ_WEB_DIR=/srv/buzz/web), so this only flips the route on — no rebuild.
    It does not expose anything new: /repos reads the same repositories the git
    smart-HTTP endpoints already serve under the same auth.

    Note that / keeps returning NIP-11 metadata even when this is true, because
    the explicit route wins over the SPA fallback. The entry point is /repos.
  EOT
  type        = bool
  default     = false
}

variable "require_relay_membership" {
  description = <<-EOT
    BUZZ_REQUIRE_RELAY_MEMBERSHIP. When true, only pubkeys in the relay's
    membership table may use the relay; NIP-42 authentication alone is not
    enough. The owner is bootstrapped as a member at startup and can then admit
    others, so enabling this does not lock you out.

    Complements the client-side host allowlist rather than duplicating it: that
    restricts where our clients may go, this restricts who may use our relay.
  EOT
  type        = bool
  default     = false

  validation {
    # buzz-relay refuses to start with membership required and no owner pubkey
    # (main.rs:228-236). Without this check, enabling membership while
    # owner_pubkey is empty applies cleanly and then crash-loops the service.
    condition     = !var.require_relay_membership || trimspace(var.owner_pubkey) != ""
    error_message = "require_relay_membership = true needs owner_pubkey set: the relay exits at startup without it (main.rs:228). Set owner_pubkey to your 64-hex Nostr pubkey."
  }
}

variable "log_level" {
  description = "RUST_LOG filter for the relay."
  type        = string
  default     = "buzz_relay=info,buzz_db=info,buzz_auth=info,tower_http=warn"
}

variable "log_retention_days" {
  description = "CloudWatch log retention."
  type        = number
  default     = 14
}

# ── Postgres ─────────────────────────────────────────────────────────────────

variable "db_engine_version" {
  description = "RDS Postgres version. 17.x matches deploy/compose (postgres:17-alpine)."
  type        = string
  default     = "17.10"
}

variable "db_instance_class" {
  description = "RDS instance class."
  type        = string
  default     = "db.t4g.micro"
}

variable "db_allocated_storage" {
  description = "RDS storage in GiB. gp3 autoscales up to db_max_allocated_storage."
  type        = number
  default     = 20
}

variable "db_max_allocated_storage" {
  description = "Storage autoscaling ceiling in GiB."
  type        = number
  default     = 100
}

variable "db_multi_az" {
  description = "Multi-AZ failover. false for the dev tier; true roughly doubles RDS cost."
  type        = bool
  default     = false
}

variable "db_backup_retention_days" {
  description = "Automated backup retention in days. 0 disables backups."
  type        = number
  default     = 7
}

# ── Redis ────────────────────────────────────────────────────────────────────

variable "redis_engine_version" {
  description = "ElastiCache Redis version."
  type        = string
  default     = "7.1"
}

variable "redis_node_type" {
  description = "ElastiCache node type."
  type        = string
  default     = "cache.t4g.micro"
}

# ── Lifecycle guards ─────────────────────────────────────────────────────────

variable "deletion_protection" {
  description = "Protect RDS from deletion. Leave false on dev so you can tear down."
  type        = bool
  default     = false
}

variable "skip_final_snapshot" {
  description = "Skip the RDS final snapshot on destroy. true is dev-only."
  type        = bool
  default     = true
}

variable "force_destroy_media_bucket" {
  description = "Allow `terraform destroy` to delete a non-empty media bucket. Dev-only."
  type        = bool
  default     = false
}

# ── Starknet ─────────────────────────────────────────────────────────────────

variable "starknet_rpc_url" {
  description = <<-EOT
    Starknet JSON-RPC endpoint, consumed by buzz-paymaster as
    BUZZ_PAYMASTER_RPC_URL.

    Renamed from `starknet_rpc_sn_main`, which fed BUZZ_STARKNET_RPC_SN_MAIN on
    the relay container for NIP-SW wallet-binding verification. NIP-SW was
    withdrawn, so no relay code has read that variable since — it was being
    injected into every task for nothing, with a comment pointing at kind:30178,
    an integer that now means something else entirely upstream. The endpoint
    itself is still wanted; only the consumer changed.

    Operator-configured on purpose: a submitted event must never influence which
    node the sponsor trusts, and the sponsor decides whether an account is already
    deployed from what this endpoint says.

    Note the hostname ordering for the public mainnet node:
    `mainnet.nodes.starknet.org`, not `starknet.nodes.org`.

    Public endpoint, so a plain env var rather than a Secrets Manager entry. If you
    move to a provider that embeds an API key in the URL, move this to the
    `secrets` block in paymaster.tf instead.
  EOT
  type        = string
  default     = ""
}
