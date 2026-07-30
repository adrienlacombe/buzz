provider "aws" {
  region = var.aws_region

  # null, not "": an empty string is still a profile name to the provider, and it
  # would fail to resolve instead of falling through to the environment
  # credentials that GitHub Actions provides via OIDC.
  profile = var.aws_profile == "" ? null : var.aws_profile

  default_tags {
    tags = {
      Project     = var.project_name
      Environment = var.environment
      ManagedBy   = "terraform"
      Source      = "infra/aws"
    }
  }
}

data "aws_availability_zones" "available" {
  state = "available"

  filter {
    name   = "opt-in-status"
    values = ["opt-in-not-required"]
  }
}

data "aws_caller_identity" "current" {}
