# FORK-LOCAL (adrienlacombe/buzz) — not present in block/buzz.
# Deploys the Buzz relay to AWS account 618867225791 (eu-west-3) on ECS Fargate.
# Upstream deploys via deploy/charts/buzz (Helm/k8s); this is an independent path
# under a new directory and touches no upstream file, so an upstream sync should
# never conflict here.

terraform {
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.6"
    }
  }

  # State lives in S3 so it survives laptop loss and stays encrypted at rest.
  # State holds the RDS password and the relay's S3 secret key in PLAINTEXT, so
  # this bucket must stay private and versioned (see bootstrap/).
  #
  # Fully specified rather than partial, so `terraform init` behaves identically
  # locally and in CI. Deliberately no `profile` here: that would name a profile
  # GitHub Actions does not have, breaking backend init. Credentials come from
  # AWS_PROFILE locally and from OIDC in CI.
  #
  #   export AWS_PROFILE=alc-tf && terraform init
  backend "s3" {
    bucket       = "buzz-tfstate-618867225791-eu-west-3"
    key          = "buzz/relay/terraform.tfstate"
    region       = "eu-west-3"
    encrypt      = true
    use_lockfile = true # S3-native locking (TF >= 1.10); no DynamoDB table needed
  }
}
