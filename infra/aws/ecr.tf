# Markets indexer container registry.
#
# Always created — not gated on indexer_enabled. An ECR repository does not
# create IAM roles that need bootstrap PassRole, so it cannot break relay CD the
# way a half-enabled paymaster/indexer did. the-situation-sdk workflow pushes
# here (assuming buzz-dev-indexer-ecr-push from bootstrap/) before the indexer
# ECS service exists; enable waits on an immutable ECR pin.
#
# Name is ${local.name}-indexer (buzz-dev-indexer). Do not use or collide with
# bim-indexer in this account — that is a different product.
#
# No GHCR pull secret. No repositoryCredentials on the task. ECS pulls with
# AmazonECSTaskExecutionRolePolicy on the indexer execution role once enabled.

resource "aws_ecr_repository" "indexer" {
  name                 = "${local.name}-indexer"
  image_tag_mutability = "IMMUTABLE"

  image_scanning_configuration {
    scan_on_push = true
  }

  tags = { Name = "${local.name}-indexer" }
}

# Keep recent sha-/version tags; expire untagged leftovers from failed pushes.
# IMMUTABLE tags mean a re-push of the same tag fails — digests pins stay valid.
resource "aws_ecr_lifecycle_policy" "indexer" {
  repository = aws_ecr_repository.indexer.name

  policy = jsonencode({
    rules = [
      {
        rulePriority = 1
        description  = "Expire untagged images after 14 days"
        selection = {
          tagStatus   = "untagged"
          countType   = "sinceImagePushed"
          countUnit   = "days"
          countNumber = 14
        }
        action = { type = "expire" }
      },
      {
        rulePriority = 2
        description  = "Keep the last 30 tagged images"
        selection = {
          tagStatus   = "any"
          countType   = "imageCountMoreThan"
          countNumber = 30
        }
        action = { type = "expire" }
      },
    ]
  })
}
