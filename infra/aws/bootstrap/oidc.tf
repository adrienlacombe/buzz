# FORK-LOCAL (adrienlacombe/buzz) — not present in block/buzz.
#
# The GitHub Actions deploy role lives in the bootstrap stack, NOT the main one,
# and that placement is the whole point of this file.
#
# It used to live in ../oidc.tf, managed by the same stack the CD pipeline
# applies. That is a bootstrap-dependency inversion: the pipeline managed the IAM
# role granting the pipeline its own access. On 2026-07-30 a CD run applied
# Terraform from a commit whose oidc.tf predated a trust-policy fix, reverted the
# policy, and every subsequent run failed with:
#
#   Not authorized to perform sts:AssumeRoleWithWebIdentity
#
# That is a deadlock rather than a transient failure — repairing the trust policy
# requires authenticating through the trust policy. Only an out-of-band local
# apply could recover it, and no amount of retrying or pushing would have.
#
# Living here fixes it structurally: this stack is applied locally and
# deliberately, with an admin profile, and the CD role has no permission to touch
# its own credentials. It sits alongside the state bucket, which is here for the
# same chicken-and-egg reason.
#
# Cross-stack references are deliberately avoided. The main stack's role and
# secret ARNs are reconstructed from their deterministic names rather than read
# via terraform_remote_state, because a remote-state dependency on the stack this
# one bootstraps would reintroduce a cycle. Renaming project_name or environment
# in the main stack means updating them here too.

data "aws_caller_identity" "current" {}

locals {
  name = "${var.project_name}-${var.environment}"

  # Reconstructed by name — see the note above on avoiding cross-stack reads.
  execution_role_arn = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${local.name}-ecs-execution"
  task_role_arn      = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${local.name}-ecs-task"

  # The paymaster runs under roles of its own rather than the relay's, so that a
  # compromise of one task cannot read the other's secrets (../paymaster.tf).
  # Without these two entries in PassExecutionAndTaskRoles below, registering its
  # task definition fails from CI with an iam:PassRole AccessDenied.
  paymaster_execution_role_arn = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${local.name}-paymaster-execution"
  paymaster_task_role_arn      = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${local.name}-paymaster-task"

  # Secrets Manager appends a random 6-character suffix, so the exact ARN is not
  # derivable from the name and this must be a prefix match.
  identity_secret_arn_pattern = "arn:aws:secretsmanager:${var.aws_region}:${data.aws_caller_identity.current.account_id}:secret:${local.name}/relay-identity-*"

  # The sponsor's Starknet signing key lives in here. Same reasoning as the relay
  # identity, with more at stake: this one spends money.
  paymaster_secret_arn_pattern = "arn:aws:secretsmanager:${var.aws_region}:${data.aws_caller_identity.current.account_id}:secret:${local.name}/paymaster-*"
}

variable "project_name" {
  description = "Must match the main stack's project_name — role names are derived from it."
  type        = string
  default     = "buzz"
}

variable "environment" {
  description = "Must match the main stack's environment."
  type        = string
  default     = "dev"
}

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

    GitHub issues subject claims containing numeric IDs rather than names for this
    repo, so a trust policy written only against the name-based form
    ("repo:owner/repo:ref:...") is rejected with:

      Not authorized to perform sts:AssumeRoleWithWebIdentity

    That is a silent failure mode — the claim looks correct in CloudTrail, which
    renders the same ID-bearing string. Read the live value with:

      gh api repos/<owner>/<repo>/actions/oidc/customization/sub

    IDs rather than names is the safer form: renaming or transferring the repo
    does not silently carry the trust with it. Both forms are trusted so a change
    on GitHub's side in either direction cannot break deploys. Set to "" to trust
    only the name-based form.
  EOT
  type        = string
  default     = "repo:adrienlacombe@6303520/buzz@1317096209"
}

# A data source, not a resource: this account already has
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
    # could assume the role.
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }

    # Both subject forms, still exact matches and still pinned to one branch —
    # no wildcards, so a PR branch or a fork cannot assume the role.
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values = compact([
        "repo:${var.github_repository}:ref:refs/heads/${var.github_deploy_branch}",
        var.github_oidc_sub_prefix_immutable != ""
        ? "${var.github_oidc_sub_prefix_immutable}:ref:refs/heads/${var.github_deploy_branch}"
        : "",
      ])
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

# The IAM half, deliberately NOT IAMFullAccess. Terraform must manage the two ECS
# roles and the relay's S3 user, so it needs real IAM write access — but scoped by
# ARN to names this stack owns. A compromised workflow therefore cannot mint an
# unrelated admin role.
#
# Note it can no longer manage the deploy role itself: that role is created here,
# and ${local.name}-github-actions falls under the ${var.project_name}-* pattern
# below, so an explicit Deny keeps CD away from its own credentials.
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
        # The structural fix. Without this, the CD role still matches
        # role/${var.project_name}-* above and could revert its own trust policy
        # or strip its own permissions, which is exactly the deadlock that moved
        # this file here. Deny beats Allow unconditionally.
        Sid    = "NeverLetCdTouchItsOwnCredentials"
        Effect = "Deny"
        Action = ["iam:*"]
        Resource = [
          aws_iam_role.github_actions.arn,
        ]
      },
      {
        # ECS needs to hand the task and execution roles to the service. Scoped by
        # service so the role cannot be passed to an arbitrary principal.
        Sid    = "PassExecutionAndTaskRoles"
        Effect = "Allow"
        Action = ["iam:PassRole"]
        Resource = [
          local.execution_role_arn,
          local.task_role_arn,
          local.paymaster_execution_role_arn,
          local.paymaster_task_role_arn,
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
        # The data source resolves a URL to an ARN, so it calls
        # ListOpenIDConnectProviders *before* GetOpenIDConnectProvider. Granting
        # only the Get denies the plan with:
        #   AccessDenied: ... not authorized to perform iam:ListOpenIDConnectProviders
        # This action takes no resource-level constraint, so "*" is the only valid
        # form. It leaks nothing beyond the existence and ARNs of the account's
        # OIDC providers.
        Sid      = "ListOidcProvidersForDataSourceLookup"
        Effect   = "Allow"
        Action   = ["iam:ListOpenIDConnectProviders"]
        Resource = "*"
      },
      {
        # PowerUserAccess already allows secretsmanager:*, and CI legitimately
        # needs to manage the runtime secret. But the relay's identity key and the
        # sponsor's Starknet signing key are never CI's business, so reading them is
        # denied outright — which is also why ../secrets.tf and ../paymaster.tf
        # deliberately do not manage those secrets' versions (Terraform reads a
        # managed version back on every refresh, which would require the Get).
        #
        # The paymaster entry is the one that matters most here: a workflow that
        # could read it could drain the sponsor's account, and every push to main
        # assumes this role.
        Sid    = "DenyIdentityAndSponsorKeyReads"
        Effect = "Deny"
        Action = ["secretsmanager:GetSecretValue"]
        Resource = [
          local.identity_secret_arn_pattern,
          local.paymaster_secret_arn_pattern,
        ]
      },
    ]
  })
}

output "github_actions_role_arn" {
  description = "Role ARN for aws-actions/configure-aws-credentials in deploy-aws.yml."
  value       = aws_iam_role.github_actions.arn
}
