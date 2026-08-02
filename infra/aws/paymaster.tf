# buzz-paymaster — sponsors Starknet fees for Nostr-key-controlled accounts.
#
# Everything paymaster-related lives in this one file so the whole service can be
# reviewed, or removed, without touching the relay stack. It shares the relay's
# cluster, subnets and image; it shares neither its security group, its IAM roles,
# nor its secrets.
#
# ── Two properties worth understanding before changing anything here ──────────
#
# 1. NO INGRESS. The paymaster connects *out* to the relay as an ordinary Nostr
#    client and subscribes. It listens on no port, has no target group, and its
#    security group has no ingress rule at all. A funded service with nothing
#    listening is a far smaller target than one exposing an authenticated API, and
#    that is the reason sponsorship is a Nostr event kind rather than an HTTP
#    endpoint. Do not "helpfully" give it a health-check port.
#
# 2. ONE TASK, EVER. The Starknet account nonce is read from the pre-confirmed
#    block and consumed by the transaction that follows, so two tasks would collide
#    on it — and both would service a request that arrived before either published
#    its result, paying twice. There is no lock. `paymaster_desired_count` is
#    validated to 0 or 1 for that reason.

# ── Variables ────────────────────────────────────────────────────────────────

variable "paymaster_desired_count" {
  description = <<-EOT
    Tasks to run. 0 or 1 only.

    Defaults to 0, which is an opt-in rather than an oversight: the service cannot
    start until the Starknet secret below holds a real funded account, and a task
    crash-looping on every CI deploy is noise that hides real failures. Set to 1
    once the account is funded and the secret is populated.
  EOT
  type        = number
  default     = 0

  validation {
    condition     = var.paymaster_desired_count == 0 || var.paymaster_desired_count == 1
    error_message = "Two paymasters collide on the account nonce and can pay twice for one request. 0 or 1."
  }
}

variable "paymaster_cpu" {
  description = "Fargate CPU units. The workload is one WebSocket and occasional JSON-RPC, so the floor is plenty."
  type        = number
  default     = 256
}

variable "paymaster_memory" {
  description = "Fargate memory (MiB). Must be a valid pairing with paymaster_cpu."
  type        = number
  default     = 512
}

variable "paymaster_account_class_hash" {
  description = <<-EOT
    BUZZ_PAYMASTER_ACCOUNT_CLASS_HASH. The NostrAccount class the sponsor deploys
    user accounts from.

    Not defaulted on purpose. It changes with any edit to
    contracts/src/account.cairo, and a stale value derives addresses no client
    would recognise — the sponsor would deploy accounts nobody can find, holding
    whatever was sent to the address the user was shown. See
    contracts/DEPLOYMENTS.md for the declared class.

    Empty leaves the service disabled regardless of paymaster_desired_count.
  EOT
  type        = string
  default     = ""
}

variable "paymaster_max_fee_fri" {
  description = <<-EOT
    BUZZ_PAYMASTER_MAX_FEE_FRI. Per-transaction fee ceiling, in Fri. Empty uses the
    binary's default of 10 STRK.

    This is a spending guard, not a per-member quota (there is deliberately no
    quota). The calls in a request are the user's and arbitrary, so a member can
    ask the sponsor to pay for something enormous; the ceiling is what bounds the
    damage from a single request. It is compared against the *padded* fee bound,
    which is 2.25x the estimate.
  EOT
  type        = string
  default     = ""
}

locals {
  # Both a class hash and a non-zero count are required. Either missing means the
  # service exists at zero tasks rather than crash-looping.
  paymaster_enabled = var.paymaster_account_class_hash != "" && var.paymaster_desired_count > 0
}

# ── Logs ─────────────────────────────────────────────────────────────────────

resource "aws_cloudwatch_log_group" "paymaster" {
  name              = "/ecs/${local.name}/paymaster"
  retention_in_days = var.log_retention_days

  tags = { Name = "${local.name}-paymaster" }
}

# ── Network ──────────────────────────────────────────────────────────────────

resource "aws_security_group" "paymaster" {
  name        = "${local.name}-paymaster"
  description = "Buzz paymaster Fargate task - egress only, listens on nothing"
  vpc_id      = aws_vpc.main.id

  tags = { Name = "${local.name}-paymaster" }

  lifecycle {
    create_before_destroy = true
  }
}

# Deliberately the only rule. See property 1 at the top of this file: there is no
# ingress rule because there is nothing to reach.
resource "aws_vpc_security_group_egress_rule" "paymaster_all" {
  security_group_id = aws_security_group.paymaster.id
  description       = "Image pull, Secrets Manager, the relay, and a Starknet RPC endpoint"
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "-1"
}

# ── Secrets ──────────────────────────────────────────────────────────────────

# The sponsor's two keys, in a secret of their own rather than the relay's.
#
# Separate from `runtime` and `identity` so the relay's execution role cannot read
# them and the paymaster's cannot read the relay's. A compromise of one task should
# not hand over the other's credentials, and with a shared secret it would.
#
# As with the relay identity, there is deliberately NO
# aws_secretsmanager_secret_version here. Terraform reads managed versions back on
# every refresh, so managing one would force the CI deploy role to hold
# secretsmanager:GetSecretValue on a key that can spend money. Leaving it unmanaged
# lets bootstrap/oidc.tf Deny that action outright.
#
# Populate it out-of-band, once:
#
#   aws secretsmanager put-secret-value \
#     --profile alc-tf --region eu-west-3 \
#     --secret-id "buzz-dev/paymaster" \
#     --secret-string '{
#       "BUZZ_PAYMASTER_NOSTR_KEY": "<hex or nsec - relay identity, spends nothing>",
#       "BUZZ_PAYMASTER_STARKNET_ADDRESS": "0x<funded sponsor account>",
#       "BUZZ_PAYMASTER_STARKNET_KEY": "0x<its signing key - spends money>"
#     }'
resource "aws_secretsmanager_secret" "paymaster" {
  name        = "${local.name}/paymaster"
  description = "Sponsor Nostr identity and Starknet signing key - set out-of-band, never managed by Terraform"

  recovery_window_in_days = 0

  tags = { Name = "${local.name}-paymaster" }
}

# ── IAM ──────────────────────────────────────────────────────────────────────

# A separate execution role, not the relay's. The relay's `execution_secrets`
# policy would have to name the paymaster secret, which would let a compromised
# relay task's role read a key that spends money.
resource "aws_iam_role" "paymaster_execution" {
  name               = "${local.name}-paymaster-execution"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json

  tags = { Name = "${local.name}-paymaster-execution" }
}

resource "aws_iam_role_policy_attachment" "paymaster_execution_managed" {
  role       = aws_iam_role.paymaster_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "paymaster_execution_secrets" {
  name = "read-paymaster-secret"
  role = aws_iam_role.paymaster_execution.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["secretsmanager:GetSecretValue"]
      Resource = [aws_secretsmanager_secret.paymaster.arn]
    }]
  })
}

# Assumed by the paymaster process itself. It needs no AWS API at all — the only
# thing here is the SSM channel that backs `aws ecs execute-command`, for reading
# state during an incident.
resource "aws_iam_role" "paymaster_task" {
  name               = "${local.name}-paymaster-task"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json

  tags = { Name = "${local.name}-paymaster-task" }
}

resource "aws_iam_role_policy" "paymaster_task_exec_channel" {
  name = "ecs-exec-ssm-channel"
  role = aws_iam_role.paymaster_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "ssmmessages:CreateControlChannel",
        "ssmmessages:CreateDataChannel",
        "ssmmessages:OpenControlChannel",
        "ssmmessages:OpenDataChannel",
      ]
      Resource = "*"
    }]
  })
}

# ── Task definition ──────────────────────────────────────────────────────────

resource "aws_ecs_task_definition" "paymaster" {
  family                   = "${local.name}-paymaster"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.paymaster_cpu
  memory                   = var.paymaster_memory
  execution_role_arn       = aws_iam_role.paymaster_execution.arn
  task_role_arn            = aws_iam_role.paymaster_task.arn

  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }

  container_definitions = jsonencode([{
    name  = "paymaster"
    image = var.relay_image
    # The same image as the relay, with the entrypoint overridden. One image means
    # one publish pipeline and one immutable :sha-<7> tag for deploy-aws.yml to
    # pin — see the FORK-LOCAL note in the root Dockerfile.
    entryPoint = ["/usr/local/bin/buzz-paymaster"]
    essential  = true

    # No portMappings, by design. See property 1 at the top of this file.

    environment = [
      # The relay this fork ships a client for, dialled from inside the VPC over
      # the public origin so NIP-42 auth and TLS work exactly as they do for a
      # desktop client.
      { name = "BUZZ_PAYMASTER_RELAY_URL", value = local.relay_url },
      { name = "BUZZ_PAYMASTER_RPC_URL", value = var.starknet_rpc_url },
      { name = "BUZZ_PAYMASTER_ACCOUNT_CLASS_HASH", value = var.paymaster_account_class_hash },
      { name = "BUZZ_PAYMASTER_MAX_FEE_FRI", value = var.paymaster_max_fee_fri },
      { name = "RUST_LOG", value = var.log_level },
    ]

    # All three from the paymaster secret. valueFrom with a trailing :key:: pulls
    # one field out of the JSON.
    secrets = [
      { name = "BUZZ_PAYMASTER_NOSTR_KEY", valueFrom = "${aws_secretsmanager_secret.paymaster.arn}:BUZZ_PAYMASTER_NOSTR_KEY::" },
      { name = "BUZZ_PAYMASTER_STARKNET_ADDRESS", valueFrom = "${aws_secretsmanager_secret.paymaster.arn}:BUZZ_PAYMASTER_STARKNET_ADDRESS::" },
      { name = "BUZZ_PAYMASTER_STARKNET_KEY", valueFrom = "${aws_secretsmanager_secret.paymaster.arn}:BUZZ_PAYMASTER_STARKNET_KEY::" },
    ]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.paymaster.name
        "awslogs-region"        = var.aws_region
        "awslogs-stream-prefix" = "paymaster"
      }
    }
  }])

  tags = { Name = "${local.name}-paymaster" }
}

# ── Service ──────────────────────────────────────────────────────────────────

resource "aws_ecs_service" "paymaster" {
  name            = "paymaster"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.paymaster.arn
  desired_count   = local.paymaster_enabled ? var.paymaster_desired_count : 0
  launch_type     = "FARGATE"

  # Public subnet with a public IP, as the relay does: needed to pull from ghcr.io
  # and to reach a Starknet RPC endpoint, and cheaper than a NAT gateway. Nothing
  # can reach *in* — the security group has no ingress rule.
  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.paymaster.id]
    assign_public_ip = true
  }

  # At one task this forces stop-then-start rather than briefly running two, which
  # would collide on the account nonce. The gap costs nothing: a request published
  # while the paymaster is down is stored on the relay and serviced on reconnect.
  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100

  # With no load balancer there is no health check but the container's own exit
  # status, which is the signal that matters here: a misconfigured sponsor exits
  # non-zero at startup rather than idling. The breaker rolls back to the last task
  # definition that stayed up.
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  enable_execute_command = true

  depends_on = [
    aws_iam_role_policy.paymaster_execution_secrets,
    aws_iam_role_policy_attachment.paymaster_execution_managed,
  ]

  lifecycle {
    # Left free so the service can be stopped by hand during an incident without
    # the next CI deploy starting it again.
    ignore_changes = [desired_count]
  }

  tags = { Name = "${local.name}-paymaster" }
}
