# GitHub Actions -> AWS via OIDC. No access keys in GitHub secrets: Actions
# exchanges a short-lived OIDC token for temporary STS credentials, so there is
# nothing long-lived to leak or rotate.
#
# The provider is a data source, not a resource: this account already has
# token.actions.githubusercontent.com registered, and IAM allows only one
# provider per URL. Creating it would fail with EntityAlreadyExists.
data "aws_iam_openid_connect_provider" "github" {
  url = "https://token.actions.githubusercontent.com"
}

data "aws_iam_policy_document" "github_assume_role" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]

    principals {
      type        = "Federated"
      identifiers = [data.aws_iam_openid_connect_provider.github.arn]
    }

    # Both conditions matter. Without the aud check any GitHub tenant could
    # present a token; without the sub check any branch or fork of this repo
    # could assume the role. sub is pinned to main specifically, so a PR branch
    # cannot deploy.
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["repo:${var.github_repository}:ref:refs/heads/${var.github_deploy_branch}"]
    }
  }
}

resource "aws_iam_role" "github_actions" {
  name        = "${local.name}-github-actions"
  description = "Assumed by GitHub Actions to deploy the Buzz relay"

  assume_role_policy   = data.aws_iam_policy_document.github_assume_role.json
  max_session_duration = 3600

  tags = { Name = "${local.name}-github-actions" }
}

# Covers everything the stack touches except IAM, which PowerUserAccess denies.
resource "aws_iam_role_policy_attachment" "github_actions_power" {
  role       = aws_iam_role.github_actions.name
  policy_arn = "arn:aws:iam::aws:policy/PowerUserAccess"
}

# The IAM half, deliberately NOT IAMFullAccess. Terraform must manage the two
# ECS roles, the relay's S3 user and this role itself, so it needs real IAM
# write access -- but scoped by ARN to names this stack owns. A compromised
# workflow therefore cannot mint an unrelated admin role.
resource "aws_iam_role_policy" "github_actions_iam" {
  name = "manage-buzz-iam-resources"
  role = aws_iam_role.github_actions.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "ManageStackRolesAndUsers"
        Effect = "Allow"
        Action = [
          "iam:GetRole",
          "iam:GetRolePolicy",
          "iam:GetUser",
          "iam:GetUserPolicy",
          "iam:ListRolePolicies",
          "iam:ListAttachedRolePolicies",
          "iam:ListUserPolicies",
          "iam:ListAccessKeys",
          "iam:ListInstanceProfilesForRole",
          "iam:ListRoleTags",
          "iam:ListUserTags",
          "iam:CreateRole",
          "iam:DeleteRole",
          "iam:UpdateRole",
          "iam:UpdateAssumeRolePolicy",
          "iam:PutRolePolicy",
          "iam:DeleteRolePolicy",
          "iam:AttachRolePolicy",
          "iam:DetachRolePolicy",
          "iam:TagRole",
          "iam:UntagRole",
          "iam:CreateUser",
          "iam:DeleteUser",
          "iam:PutUserPolicy",
          "iam:DeleteUserPolicy",
          "iam:TagUser",
          "iam:UntagUser",
          "iam:CreateAccessKey",
          "iam:DeleteAccessKey",
          "iam:UpdateAccessKey",
        ]
        Resource = [
          "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${var.project_name}-*",
          "arn:aws:iam::${data.aws_caller_identity.current.account_id}:user/service/${var.project_name}-*",
        ]
      },
      {
        # ECS needs to hand the task and execution roles to the service. Scoped
        # by service so the role cannot be passed to an arbitrary principal.
        Sid    = "PassExecutionAndTaskRoles"
        Effect = "Allow"
        Action = ["iam:PassRole"]
        Resource = [
          aws_iam_role.execution.arn,
          aws_iam_role.task.arn,
        ]
        Condition = {
          StringEquals = { "iam:PassedToService" = "ecs-tasks.amazonaws.com" }
        }
      },
      {
        # Terraform refreshes the OIDC data source on every plan.
        Sid      = "ReadOidcProvider"
        Effect   = "Allow"
        Action   = ["iam:GetOpenIDConnectProvider"]
        Resource = data.aws_iam_openid_connect_provider.github.arn
      },
      {
        # PowerUserAccess already allows s3:*, but Secrets Manager values are
        # what CI must NOT be able to read. It needs to manage the secret
        # containers and versions; it never needs GetSecretValue, so that is
        # left out and the relay's identity key stays unreadable from CI.
        Sid    = "DenySecretValueReads"
        Effect = "Deny"
        Action = ["secretsmanager:GetSecretValue"]
        Resource = [
          aws_secretsmanager_secret.identity.arn,
        ]
      },
    ]
  })
}
