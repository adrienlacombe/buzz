# buzz-avnu-proxy — public HTTPS front for AVNU's SNIP-29 paymaster.
#
# Clients call this host; the proxy injects AVNU_API_KEY server-side. Shipped
# clients refuse loopback AVNU_PROXY_URL (crates/buzz-core/src/markets.rs
# resolve_avnu_proxy_url), so a public host is required before Wallet can set
# the product URL. Do NOT bake AVNU_API_KEY into the image, repo, or client.
#
# This is NOT paymaster.tf. That file is the old Nostr/STRK sponsor
# (buzz-paymaster): egress-only, no inbound, wrong product. Leave
# paymaster_enabled = false. Hostname paymaster.<domain> is deliberate product
# naming for the AVNU proxy path.
#
# Everything avnu-proxy-related that can live in one file does, so the whole
# service can be reviewed or removed without touching the relay stack. It
# shares the relay's cluster, VPC, public subnets, ALB, and container *image*;
# it shares neither the relay security group, the relay IAM roles, the relay
# secret, RDS, Redis, nor the git EFS.
#
# ── Properties worth understanding before changing anything here ─────────────
#
# 1. OFF MEANS NO RESOURCES ON CREATE. Every resource in *this* file is gated
#    on `count = var.avnu_proxy_enabled ? 1 : 0`. Same lesson as paymaster.tf /
#    indexer.tf: creating IAM roles while desired_count sat at 0 still broke
#    the next CI relay deploy with iam:PassRole, because bootstrap/ had not
#    been applied. Adding an optional service must never be able to break the
#    relay's pipeline.
#
# 2. HTTP INGRESS, UNLIKE PAYMASTER. Public HTTPS API. Security group allows
#    ingress from the ALB on port 8788 only. Host-header routing on the shared
#    HTTPS listener forwards Host paymaster.<domain> here; the listener's
#    default action stays the relay. Do not point the default action or the
#    relay /_readiness health check at this service.
#
# 3. IMAGE IS var.relay_image — ONE CD PIN. The binary ships in the relay
#    image (`/usr/local/bin/buzz-avnu-proxy`; Dockerfile already builds and
#    copies it). The task uses `command = ["/usr/local/bin/buzz-avnu-proxy"]`
#    and `image = var.relay_image`. There is deliberately no separate
#    avnu_proxy_image variable: CD already passes relay_image on every apply,
#    so one pin updates both the relay and this proxy. Indexer does the
#    opposite (own ECR image) because it is a different artefact — do not copy
#    that here, and do not invent a second image writer that could un-pin CD.
#
# 4. SECRET ALREADY EXISTS. AWS already holds buzz-dev/avnu-proxy (unmanaged
#    version) with JSON keys AVNU_API_KEY and PROXY_AUTH_TOKEN. This file
#    creates aws_secretsmanager_secret so IAM can reference it, but
#    deliberately NO aws_secretsmanager_secret_version — same reason as
#    indexer/paymaster: the CI deploy role must not GetSecretValue. First
#    enable imports the existing secret (see variable description / README).

# ── Variables ────────────────────────────────────────────────────────────────

variable "avnu_proxy_enabled" {
  description = <<-EOT
    Whether to create the buzz-avnu-proxy stack at all. Default false.

    Gates every resource in this file, not just how many tasks run. Off means
    no resources on the *create* path — see property 1 at the top of this file.

    Turning it on, in this order (do not flip enabled until bootstrap is
    applied; do not set enabled=true in a PR that only lands the stack):

      1. Apply `bootstrap/` — grants iam:PassRole on the two avnu-proxy roles
         below and extends the GetSecretValue Deny to buzz-dev/avnu-proxy.
         Separate state; CI never applies it. Prefer --profile alc for AWS CLI;
         terraform aws_profile in committed tfvars stays alc-tf:
           terraform -chdir=infra/aws/bootstrap apply
      2. Set avnu_proxy_enabled = true with avnu_proxy_desired_count = 0.
      3. Import the existing secret (look up the live ARN at apply time — do
         not hardcode the random suffix; name is buzz-dev/avnu-proxy,
         account 618867225791, eu-west-3):
           aws secretsmanager describe-secret \
             --profile alc --region eu-west-3 \
             --secret-id buzz-dev/avnu-proxy \
             --query ARN --output text
           terraform import -var-file=dev.tfvars \
             -var relay_image="$(…current relay image…)" \
             'aws_secretsmanager_secret.avnu_proxy[0]' <ARN>
      4. Apply the main stack (creates SG, IAM, TG, listener rule, Route53,
         task def, service at desired_count 0).
      5. Set avnu_proxy_desired_count = 1 and apply again.

    Doing (2)/(4) before (1) reproduces the paymaster PassRole failure on the
    next relay CD apply. Skipping (3) makes Terraform try to create a secret
    that already exists.
  EOT
  type        = bool
  default     = false
}

variable "avnu_proxy_cpu" {
  description = "Fargate CPU units for the avnu-proxy task."
  type        = number
  default     = 256
}

variable "avnu_proxy_memory" {
  description = "Fargate memory (MiB). Must be a valid pairing with avnu_proxy_cpu."
  type        = number
  default     = 512
}

variable "avnu_proxy_desired_count" {
  description = <<-EOT
    Tasks to run when the avnu-proxy is enabled. 0 or 1 only.

    Defaults to 0 so enabling the stack can import the secret and create IAM
    roles without starting a task. Set to 1 once bootstrap is applied, the
    secret is imported, and BIND_ADDR / PROXY_AUTH_TOKEN are confirmed in the
    existing secret.
  EOT
  type        = number
  default     = 0

  validation {
    condition     = var.avnu_proxy_desired_count == 0 || var.avnu_proxy_desired_count == 1
    error_message = "avnu-proxy desired_count is 0 or 1 only."
  }
}

locals {
  avnu_proxy_port = 8788

  # Resources exist iff avnu_proxy_enabled; a task actually runs only when
  # desired_count > 0. Image is always var.relay_image (required on every
  # main-stack apply), so there is no empty-image half-config like indexer.
  avnu_proxy_running = var.avnu_proxy_enabled && var.avnu_proxy_desired_count > 0
}

# ── Logs ─────────────────────────────────────────────────────────────────────

resource "aws_cloudwatch_log_group" "avnu_proxy" {
  count = var.avnu_proxy_enabled ? 1 : 0

  name              = "/ecs/${local.name}/avnu-proxy"
  retention_in_days = var.log_retention_days

  tags = { Name = "${local.name}-avnu-proxy" }
}

# ── Network ──────────────────────────────────────────────────────────────────

resource "aws_security_group" "avnu_proxy" {
  count = var.avnu_proxy_enabled ? 1 : 0

  name        = "${local.name}-avnu-proxy"
  description = "Buzz AVNU proxy Fargate task - ALB ingress on 8788 only"
  vpc_id      = aws_vpc.main.id

  tags = { Name = "${local.name}-avnu-proxy" }

  lifecycle {
    create_before_destroy = true

    precondition {
      condition     = !var.avnu_proxy_enabled || local.enable_dns
      error_message = "avnu_proxy_enabled needs domain_name set: host-header routing for paymaster.<domain> requires the HTTPS listener."
    }
  }
}

resource "aws_vpc_security_group_ingress_rule" "avnu_proxy_from_alb" {
  count = var.avnu_proxy_enabled ? 1 : 0

  security_group_id            = aws_security_group.avnu_proxy[0].id
  description                  = "AVNU proxy HTTP from the ALB only"
  referenced_security_group_id = aws_security_group.alb.id
  from_port                    = local.avnu_proxy_port
  to_port                      = local.avnu_proxy_port
  ip_protocol                  = "tcp"
}

resource "aws_vpc_security_group_egress_rule" "avnu_proxy_all" {
  count = var.avnu_proxy_enabled ? 1 : 0

  security_group_id = aws_security_group.avnu_proxy[0].id
  description       = "Image pull, Secrets Manager, AVNU upstream HTTPS"
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "-1"
}

# ALB -> avnu-proxy (standalone rules, same pattern as alb_to_relay / alb_to_indexer).
resource "aws_vpc_security_group_egress_rule" "alb_to_avnu_proxy" {
  count = var.avnu_proxy_enabled ? 1 : 0

  security_group_id            = aws_security_group.alb.id
  description                  = "Forward traffic to the AVNU proxy"
  referenced_security_group_id = aws_security_group.avnu_proxy[0].id
  from_port                    = local.avnu_proxy_port
  to_port                      = local.avnu_proxy_port
  ip_protocol                  = "tcp"
}

# ── Secrets ──────────────────────────────────────────────────────────────────

# AVNU_API_KEY + PROXY_AUTH_TOKEN in a secret of their own rather than the
# relay's. Separate so the relay's execution role cannot read them and this
# proxy's cannot read the relay's.
#
# Deliberately NO aws_secretsmanager_secret_version here — same reason as
# paymaster.tf / indexer.tf / secrets.tf identity: Terraform reading a managed
# version on every refresh would force the CI deploy role to hold
# GetSecretValue on the AVNU key. Leaving the version unmanaged lets
# bootstrap/oidc.tf Deny that action.
#
# The secret already exists in AWS (name buzz-dev/avnu-proxy). First enable
# must `terraform import` it — see avnu_proxy_enabled description. Do not
# put-secret-value from this repo; values never land in git.
resource "aws_secretsmanager_secret" "avnu_proxy" {
  count = var.avnu_proxy_enabled ? 1 : 0

  name        = "${local.name}/avnu-proxy"
  description = "AVNU_API_KEY and PROXY_AUTH_TOKEN for buzz-avnu-proxy - set out-of-band, never managed by Terraform"

  # 7-day recovery window so a disable/destroy does not immediately drop the
  # credential. Version stays unmanaged — no aws_secretsmanager_secret_version.
  recovery_window_in_days = 7

  tags = { Name = "${local.name}-avnu-proxy" }
}

# ── IAM ──────────────────────────────────────────────────────────────────────

resource "aws_iam_role" "avnu_proxy_execution" {
  count = var.avnu_proxy_enabled ? 1 : 0

  name               = "${local.name}-avnu-proxy-execution"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json

  tags = { Name = "${local.name}-avnu-proxy-execution" }
}

resource "aws_iam_role_policy_attachment" "avnu_proxy_execution_managed" {
  count = var.avnu_proxy_enabled ? 1 : 0

  role       = aws_iam_role.avnu_proxy_execution[0].name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "avnu_proxy_execution_secrets" {
  count = var.avnu_proxy_enabled ? 1 : 0

  name = "read-avnu-proxy-secret"
  role = aws_iam_role.avnu_proxy_execution[0].id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["secretsmanager:GetSecretValue"]
      Resource = [aws_secretsmanager_secret.avnu_proxy[0].arn]
    }]
  })
}

resource "aws_iam_role" "avnu_proxy_task" {
  count = var.avnu_proxy_enabled ? 1 : 0

  name               = "${local.name}-avnu-proxy-task"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json

  tags = { Name = "${local.name}-avnu-proxy-task" }
}

resource "aws_iam_role_policy" "avnu_proxy_task_exec_channel" {
  count = var.avnu_proxy_enabled ? 1 : 0

  name = "ecs-exec-ssm-channel"
  role = aws_iam_role.avnu_proxy_task[0].id

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

# ── Target group + host-header rule ──────────────────────────────────────────

resource "aws_lb_target_group" "avnu_proxy" {
  count = var.avnu_proxy_enabled ? 1 : 0

  name        = "${local.name}-avnu-proxy"
  port        = local.avnu_proxy_port
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = aws_vpc.main.id

  # Health: GET /health returns {"status":"ok","service":"buzz-avnu-proxy"}.
  # JSON-RPC: POST / and POST /rpc (Bearer PROXY_AUTH_TOKEN when off-loopback).
  # Do NOT reuse the relay's /_readiness probe or health port 8080.
  health_check {
    enabled             = true
    path                = "/health"
    port                = "traffic-port"
    protocol            = "HTTP"
    matcher             = "200"
    interval            = 15
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
  }

  deregistration_delay = 30

  lifecycle {
    create_before_destroy = true
  }

  tags = { Name = "${local.name}-avnu-proxy" }
}

# Priority 110 — indexer markets host is 100. Default action remains the relay.
resource "aws_lb_listener_rule" "avnu_proxy_paymaster_host" {
  count = var.avnu_proxy_enabled ? 1 : 0

  listener_arn = aws_lb_listener.https[0].arn
  priority     = 110

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.avnu_proxy[0].arn
  }

  condition {
    host_header {
      values = [local.paymaster_fqdn]
    }
  }

  tags = { Name = "${local.name}-avnu-proxy-paymaster-host" }
}

# ── Task definition ──────────────────────────────────────────────────────────

resource "aws_ecs_task_definition" "avnu_proxy" {
  count = var.avnu_proxy_enabled ? 1 : 0

  family                   = "${local.name}-avnu-proxy"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.avnu_proxy_cpu
  memory                   = var.avnu_proxy_memory
  execution_role_arn       = aws_iam_role.avnu_proxy_execution[0].arn
  task_role_arn            = aws_iam_role.avnu_proxy_task[0].arn

  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }

  container_definitions = jsonencode([{
    name = "avnu-proxy"
    # Same relay image CD pins — one writer. Binary is already in the image.
    image     = var.relay_image
    essential = true

    # ENTRYPOINT stays the relay image default; override only the command so
    # this task runs buzz-avnu-proxy instead of buzz-relay.
    command = ["/usr/local/bin/buzz-avnu-proxy"]

    portMappings = [
      { containerPort = local.avnu_proxy_port, protocol = "tcp" },
    ]

    environment = [
      # Non-loopback bind requires PROXY_AUTH_TOKEN (already in the secret).
      { name = "BIND_ADDR", value = "0.0.0.0:${local.avnu_proxy_port}" },
    ]

    # valueFrom with a trailing :key:: pulls one field out of the JSON secret.
    secrets = [
      { name = "AVNU_API_KEY", valueFrom = "${aws_secretsmanager_secret.avnu_proxy[0].arn}:AVNU_API_KEY::" },
      { name = "PROXY_AUTH_TOKEN", valueFrom = "${aws_secretsmanager_secret.avnu_proxy[0].arn}:PROXY_AUTH_TOKEN::" },
    ]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.avnu_proxy[0].name
        "awslogs-region"        = var.aws_region
        "awslogs-stream-prefix" = "avnu-proxy"
      }
    }
  }])

  tags = { Name = "${local.name}-avnu-proxy" }
}

# ── Service ──────────────────────────────────────────────────────────────────

resource "aws_ecs_service" "avnu_proxy" {
  count = var.avnu_proxy_enabled ? 1 : 0

  name            = "avnu-proxy"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.avnu_proxy[0].arn
  desired_count   = local.avnu_proxy_running ? var.avnu_proxy_desired_count : 0
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.avnu_proxy[0].id]
    assign_public_ip = true
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.avnu_proxy[0].arn
    container_name   = "avnu-proxy"
    container_port   = local.avnu_proxy_port
  }

  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100

  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  health_check_grace_period_seconds = 60

  enable_execute_command = true

  depends_on = [
    aws_lb_listener_rule.avnu_proxy_paymaster_host,
    aws_iam_role_policy.avnu_proxy_execution_secrets,
    aws_iam_role_policy_attachment.avnu_proxy_execution_managed,
  ]

  # Deliberately does NOT copy paymaster's ignore_changes = [desired_count].
  # Enable is two-phase: create at desired_count = 0, then set 1 and apply.
  # Ignoring desired_count would make that second apply a no-op.

  tags = { Name = "${local.name}-avnu-proxy" }
}
