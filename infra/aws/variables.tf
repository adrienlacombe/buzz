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

# ── CI/CD ────────────────────────────────────────────────────────────────────

variable "github_repository" {
  description = "owner/repo allowed to assume the deploy role via OIDC."
  type        = string
  default     = "adrienlacombe/buzz"
}

variable "github_deploy_branch" {
  description = "Only this branch may assume the deploy role. PR branches cannot deploy."
  type        = string
  default     = "main"
}

variable "github_oidc_sub_prefix_immutable" {
  description = <<-EOT
    GitHub's ID-based OIDC subject prefix for this repository, of the form
    "repo:<owner>@<account_id>/<repo>@<repo_id>".

    GitHub issues subject claims containing numeric IDs rather than names for
    this repo, so a trust policy written only against the name-based form
    ("repo:owner/repo:ref:...") is rejected with:

      Not authorized to perform sts:AssumeRoleWithWebIdentity

    That is a silent failure mode -- the claim looks correct in CloudTrail,
    which renders the same ID-bearing string. Read the live value with:

      gh api repos/<owner>/<repo>/actions/oidc/customization/sub

    IDs rather than names is the safer form: renaming or transferring the repo
    does not silently carry the trust with it. Both forms are trusted so a change
    on GitHub's side in either direction cannot break deploys. Set to "" to trust
    only the name-based form.
  EOT
  type        = string
  default     = "repo:adrienlacombe@6303520/buzz@1317096209"
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
  description = "Relay container image. Public GHCR image needs no pull secret."
  type        = string
  default     = "ghcr.io/block/buzz:main"
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

# ── NIP-SW Starknet wallet bindings ──────────────────────────────────────────

variable "starknet_rpc_sn_main" {
  description = <<-EOT
    BUZZ_STARKNET_RPC_SN_MAIN. Starknet mainnet JSON-RPC endpoint the relay calls
    to verify NIP-SW wallet-binding attestations (kind:30178) at ingest.

    Verification FAILS CLOSED. Left empty, the relay rejects every SN_MAIN
    binding rather than storing one it could not check — the intended default,
    since a stored binding is supposed to mean an attested one.

    Operator-configured on purpose. A submitted event must never influence which
    endpoint is trusted, or an attacker could point verification at a node that
    answers VALID to everything.

    Public endpoint, so a plain env var rather than a Secrets Manager entry. If
    you move to a provider that embeds an API key in the URL, move this to the
    `secrets` block in ecs.tf instead.
  EOT
  type        = string
  default     = ""
}
