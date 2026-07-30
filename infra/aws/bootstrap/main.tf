# Creates the S3 bucket that holds the main stack's Terraform state.
#
# Chicken-and-egg: this one stack keeps its own state locally, because the bucket
# it creates is where every other stack's state goes. It is ~30 lines and changes
# roughly never, so a local state file here is an acceptable trade.
#
#   terraform init && terraform apply
#   cd .. && terraform init -backend-config=backend.hcl

terraform {
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    local = {
      source  = "hashicorp/local"
      version = "~> 2.5"
    }
  }
}

provider "aws" {
  region  = var.aws_region
  profile = var.aws_profile

  default_tags {
    tags = {
      Project   = "buzz"
      ManagedBy = "terraform"
      Source    = "infra/aws/bootstrap"
    }
  }
}

variable "aws_region" {
  type    = string
  default = "eu-west-3"
}

variable "aws_profile" {
  type    = string
  default = "alc-tf"
}

variable "state_bucket_name" {
  description = "Globally unique bucket name for Terraform state."
  type        = string
  default     = "buzz-tfstate-618867225791-eu-west-3"
}

resource "aws_s3_bucket" "state" {
  bucket = var.state_bucket_name

  # No force_destroy on purpose. Losing state means losing the ability to manage
  # or cleanly destroy everything else, so this should be hard to delete.

  tags = { Name = "buzz-tfstate" }
}

resource "aws_s3_bucket_public_access_block" "state" {
  bucket = aws_s3_bucket.state.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# State contains the RDS password and the relay's S3 secret key in plaintext.
# Versioning is what makes a corrupted or truncated state recoverable.
resource "aws_s3_bucket_versioning" "state" {
  bucket = aws_s3_bucket.state.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "state" {
  bucket = aws_s3_bucket.state.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
    bucket_key_enabled = true
  }
}

# Reject any plaintext HTTP request to the state bucket.
resource "aws_s3_bucket_policy" "state_tls_only" {
  bucket = aws_s3_bucket.state.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "DenyInsecureTransport"
      Effect    = "Deny"
      Principal = "*"
      Action    = "s3:*"
      Resource = [
        aws_s3_bucket.state.arn,
        "${aws_s3_bucket.state.arn}/*",
      ]
      Condition = {
        Bool = { "aws:SecureTransport" = "false" }
      }
    }]
  })

  depends_on = [aws_s3_bucket_public_access_block.state]
}

output "state_bucket" {
  value = aws_s3_bucket.state.id
}

# The parent stack hardcodes this bucket in its backend block (see ../versions.tf)
# so that init needs no generated config file and behaves the same in CI. If you
# change state_bucket_name, update that block to match.
output "next_step" {
  value = "cd .. && export AWS_PROFILE=${var.aws_profile} && terraform init"
}
