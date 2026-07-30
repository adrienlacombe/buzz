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
