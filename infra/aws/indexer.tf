# the-situation indexer — HTTP API + SQLite ETL for Bitcoin Markets prediction
# markets at markets.bitcoinmarkets.app.
#
# Everything indexer-related that can live in one file does, so the whole service
# can be reviewed or removed without touching the relay stack. It shares the
# relay's cluster, VPC, public subnets and ALB; it shares neither RDS, Redis, the
# relay security group, the relay IAM roles, the relay secret, nor the git EFS.
#
# Container image is @the-situation/indexer (npm), NOT the relay image. Do not
# fold this into the relay process or Dockerfile.
#
# ── Two properties worth understanding before changing anything here ──────────
#
# 1. OFF MEANS NO RESOURCES ON CREATE. Every resource in *this* file is gated on
#    `count = var.indexer_enabled ? 1 : 0`. That distinction was learned the hard
#    way on paymaster.tf: creating IAM roles while desired_count sat at 0 still
#    broke the next CI relay deploy with iam:PassRole, because bootstrap/ had not
#    been applied. Adding an optional service must never be able to break the
#    relay's pipeline. "Off means no resources" is the *create* path — once the
#    indexer EFS exists, flipping enabled back to false is not a silent teardown
#    (see indexer_enabled description / prevent_destroy below).
#
#    The ECR repository is the deliberate exception: it lives in ecr.tf and is
#    always created so the-situation-sdk can push before ECS exists. An ECR repo
#    creates no PassRole surface, so it cannot break relay CD.
#
# 2. HTTP INGRESS, UNLIKE PAYMASTER. The indexer is a public HTTPS API. Its
#    security group allows ingress from the ALB on port 8787 only. Host-header
#    routing on the shared HTTPS listener forwards Host markets.<domain> here;
#    the listener's default action stays the relay. Do not point the default
#    action or the relay /_readiness health check at this service.
#
# 3. IMAGE FROM ECR, NOT GHCR. Anonymous GHCR pull of the private
#    the-situation-sdk/indexer package is 401. Fine-grained PATs cannot do GitHub
#    Packages; we will not mint a classic PAT or a buzz-dev/indexer-ghcr pull
#    secret. Pin indexer_image to the ECR URI@digest after the SDK workflow
#    pushes (role arn:aws:iam::618867225791:role/buzz-dev-indexer-ecr-push).
#    ECS pulls with AmazonECSTaskExecutionRolePolicy alone — no
#    repositoryCredentials.

# ── Variables ────────────────────────────────────────────────────────────────

variable "indexer_enabled" {
  description = <<-EOT
    Whether to create the markets indexer stack at all. Default false.

    Gates every resource in this file, not just how many tasks run. Off means no
    resources on the *create* path — see property 1 at the top of this file.
    (The ECR repository in ecr.tf is always present so images can land first.)

    Once enabled, setting indexer_enabled = false will fail plan because
    aws_efs_file_system.indexer carries lifecycle.prevent_destroy. That is
    intentional so CD cannot wipe the markets SQLite. To disable after enable
    you must first:

      terraform state rm 'aws_efs_file_system.indexer[0]'

    (and accept losing the count-gated resources; the filesystem then sits
    orphaned in AWS until deleted by hand). This is not a silent destroy of an
    existing DB.

    Turning it on, in this order (do not enable until an ECR pin exists):

      1. Apply `bootstrap/` — creates buzz-dev-indexer-ecr-push (SDK OIDC push),
         grants iam:PassRole on the two indexer roles below, and extends the
         GetSecretValue Deny to the indexer secret. Separate state; CI never
         applies it. Prefer --profile alc for AWS CLI; terraform aws_profile
         in committed tfvars stays alc-tf:
           terraform -chdir=infra/aws/bootstrap apply
      2. Main stack already creates aws_ecr_repository.indexer (even while this
         flag is false). After merge/apply, the-situation-sdk workflow on main
         pushes to ECR assuming
         arn:aws:iam::618867225791:role/buzz-dev-indexer-ecr-push.
      3. Set indexer_image to the ECR URI@digest from that push, then
         indexer_enabled = true with indexer_desired_count = 0, and populate
         the unmanaged secret (command below).

    Doing (3) before (1) reproduces the paymaster PassRole failure on the next
    relay CD apply. Doing (3) before an ECR image exists leaves ECS unable to
    pull — there is no GHCR fallback and no pull secret.
  EOT
  type        = bool
  default     = false
}

variable "indexer_image" {
  description = <<-EOT
    Indexer container image (@the-situation/indexer), NOT var.relay_image.

    Required when enabling the service. Empty is allowed while indexer_enabled
    is false so relay CD (which only passes relay_image) keeps working. A
    precondition on the security group fails the plan if you enable without an
    image.

    Deliberately rejects mutable :main / :latest tags — same rule as
    relay_image — so a local apply cannot quietly un-pin what is running.

    Pin from Amazon ECR after the first successful SDK → ECR push (tag
    immutability + digest). Enable waits on this form — do not use GHCR:

      indexer_image = "618867225791.dkr.ecr.eu-west-3.amazonaws.com/buzz-dev-indexer:<immutable>@sha256:<digest>"

    Historical GHCR note only (private package; anonymous pull 401; not an
    enable pin): ghcr.io/adrienlacombe/the-situation-sdk/indexer:0.19.1@sha256:c41cf55281c2060e306d05feb108b1867473edf4dac11a223251b2fc5e0bc596
    Do not make that package public. Do not mint a classic PAT. Do not add
    repositoryCredentials or buzz-dev/indexer-ghcr.
  EOT
  type        = string
  default     = ""

  validation {
    condition     = var.indexer_image == "" || !can(regex(":(main|latest)$", var.indexer_image))
    error_message = "indexer_image must not use a mutable tag (:main, :latest) — pass an immutable ECR tag@digest (e.g. 618867225791.dkr.ecr.eu-west-3.amazonaws.com/buzz-dev-indexer:<tag>@sha256:…). Leave empty while indexer_enabled is false."
  }
}

variable "indexer_cpu" {
  description = "Fargate CPU units for the indexer task."
  type        = number
  default     = 256
}

variable "indexer_memory" {
  description = "Fargate memory (MiB). Must be a valid pairing with indexer_cpu."
  type        = number
  default     = 512
}

variable "indexer_desired_count" {
  description = <<-EOT
    Tasks to run when the indexer is enabled and an image is set. 0 or 1 only.

    Defaults to 0 so enabling the stack can create the unmanaged secret and IAM
    roles without starting a task that would crash-loop until put-secret-value.
    Set to 1 once the secret is populated and indexer_image is pinned.
  EOT
  type        = number
  default     = 0

  validation {
    condition     = var.indexer_desired_count == 0 || var.indexer_desired_count == 1
    error_message = "Indexer SQLite on one EFS volume is not multi-writer safe. 0 or 1."
  }
}

locals {
  indexer_port      = 8787
  indexer_data_path = "/var/lib/situation-indexer"
  indexer_db_path   = "${local.indexer_data_path}/indexer.db"

  # Public mainnet RPC used when var.starknet_rpc_url is unset. Same hostname
  # ordering note as variables.tf: mainnet.nodes.starknet.org.
  indexer_starknet_rpc_url = var.starknet_rpc_url != "" ? var.starknet_rpc_url : "https://mainnet.nodes.starknet.org/rpc/v0_10"

  # Resources exist iff indexer_enabled; a task actually runs only when an image
  # is set too, so a half-configured stack sits at zero rather than crash-looping.
  indexer_running = var.indexer_enabled && var.indexer_image != "" && var.indexer_desired_count > 0
}

# ── Logs ─────────────────────────────────────────────────────────────────────

resource "aws_cloudwatch_log_group" "indexer" {
  count = var.indexer_enabled ? 1 : 0

  name              = "/ecs/${local.name}/indexer"
  retention_in_days = var.log_retention_days

  tags = { Name = "${local.name}-indexer" }
}

# ── Network ──────────────────────────────────────────────────────────────────

resource "aws_security_group" "indexer" {
  count = var.indexer_enabled ? 1 : 0

  name        = "${local.name}-indexer"
  description = "Buzz markets indexer Fargate task - ALB ingress on 8787 only"
  vpc_id      = aws_vpc.main.id

  tags = { Name = "${local.name}-indexer" }

  lifecycle {
    create_before_destroy = true

    precondition {
      condition     = !var.indexer_enabled || var.indexer_image != ""
      error_message = "indexer_enabled needs indexer_image set to an immutable tag (not :main/:latest)."
    }

    precondition {
      condition     = !var.indexer_enabled || local.enable_dns
      error_message = "indexer_enabled needs domain_name set: host-header routing for markets.<domain> requires the HTTPS listener."
    }
  }
}

resource "aws_vpc_security_group_ingress_rule" "indexer_from_alb" {
  count = var.indexer_enabled ? 1 : 0

  security_group_id            = aws_security_group.indexer[0].id
  description                  = "Indexer HTTP from the ALB only"
  referenced_security_group_id = aws_security_group.alb.id
  from_port                    = local.indexer_port
  to_port                      = local.indexer_port
  ip_protocol                  = "tcp"
}

resource "aws_vpc_security_group_egress_rule" "indexer_all" {
  count = var.indexer_enabled ? 1 : 0

  security_group_id = aws_security_group.indexer[0].id
  description       = "Image pull, Secrets Manager, Starknet RPC, Voyager API"
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "-1"
}

# ALB -> indexer (standalone rules, same pattern as alb_to_relay in security.tf).
resource "aws_vpc_security_group_egress_rule" "alb_to_indexer" {
  count = var.indexer_enabled ? 1 : 0

  security_group_id            = aws_security_group.alb.id
  description                  = "Forward traffic to the markets indexer"
  referenced_security_group_id = aws_security_group.indexer[0].id
  from_port                    = local.indexer_port
  to_port                      = local.indexer_port
  ip_protocol                  = "tcp"
}

# ── EFS (own filesystem — do not mount the relay git volume) ─────────────────

resource "aws_efs_file_system" "indexer" {
  count = var.indexer_enabled ? 1 : 0

  creation_token  = "${local.name}-indexer"
  encrypted       = true
  throughput_mode = "bursting"

  lifecycle_policy {
    transition_to_ia = "AFTER_30_DAYS"
  }

  tags = { Name = "${local.name}-indexer" }

  lifecycle {
    # Holds the markets SQLite DB. Unattended CI must never replace it.
    # Combined with count = indexer_enabled: flipping enabled back to false
    # fails plan rather than destroying the filesystem — intentional. See the
    # indexer_enabled variable description for the state-rm escape hatch.
    prevent_destroy = true
  }
}

resource "aws_security_group" "indexer_efs" {
  count = var.indexer_enabled ? 1 : 0

  name        = "${local.name}-indexer-efs"
  description = "EFS indexer SQLite storage - indexer tasks only"
  vpc_id      = aws_vpc.main.id

  tags = { Name = "${local.name}-indexer-efs" }
}

resource "aws_vpc_security_group_ingress_rule" "indexer_efs_from_indexer" {
  count = var.indexer_enabled ? 1 : 0

  security_group_id            = aws_security_group.indexer_efs[0].id
  description                  = "NFS"
  referenced_security_group_id = aws_security_group.indexer[0].id
  from_port                    = 2049
  to_port                      = 2049
  ip_protocol                  = "tcp"
}

resource "aws_efs_mount_target" "indexer" {
  count = var.indexer_enabled ? 2 : 0

  file_system_id  = aws_efs_file_system.indexer[0].id
  subnet_id       = aws_subnet.private[count.index].id
  security_groups = [aws_security_group.indexer_efs[0].id]
}

# Pin ownership so the container can create indexer.db regardless of whether the
# image runs as root or a non-root user. 1000 matches the relay convention.
resource "aws_efs_access_point" "indexer" {
  count = var.indexer_enabled ? 1 : 0

  file_system_id = aws_efs_file_system.indexer[0].id

  posix_user {
    uid = local.container_uid
    gid = local.container_gid
  }

  root_directory {
    path = "/indexer"

    creation_info {
      owner_uid   = local.container_uid
      owner_gid   = local.container_gid
      permissions = "0755"
    }
  }

  tags = { Name = "${local.name}-indexer" }
}

# ── Secrets ──────────────────────────────────────────────────────────────────

# ADMIN_API_KEY (and VOYAGER_API_KEY, required for boot) in a secret of their
# own rather than the relay's. Separate so the relay's execution role cannot
# read them and the indexer's cannot read the relay's.
#
# Deliberately NO aws_secretsmanager_secret_version here — same reason as
# paymaster.tf / secrets.tf identity: Terraform reading a managed version on
# every refresh would force the CI deploy role to hold GetSecretValue on the
# admin key. Leaving it unmanaged lets bootstrap/oidc.tf Deny that action.
#
# Populate it out-of-band, once (keys shown; never commit values). Prefer
# --profile alc for operator CLI; committed terraform aws_profile stays alc-tf:
#
#   aws secretsmanager put-secret-value \
#     --profile alc --region eu-west-3 \
#     --secret-id "buzz-dev/indexer" \
#     --secret-string '{
#       "ADMIN_API_KEY": "<generate and keep offline>",
#       "VOYAGER_API_KEY": "<voyager explorer API key, or a dummy to boot>"
#     }'
#
# Required to start (0.19.1): ADMIN_API_KEY, and VOYAGER_API_KEY or
# VOYAGER_API_KEYS. Voyager is NOT needed for GET /api/markets after an admin
# POST (that path is SQLite only) — a dummy key lets the process boot; a real
# key is needed for event poll. Prefer VOYAGER_API_KEYS (comma-separated pool)
# in the secret instead of VOYAGER_API_KEY if you want rotation; wire that env
# name in the secrets block if you switch.
resource "aws_secretsmanager_secret" "indexer" {
  count = var.indexer_enabled ? 1 : 0

  name        = "${local.name}/indexer"
  description = "Indexer ADMIN_API_KEY and VOYAGER_API_KEY - set out-of-band, never managed by Terraform"

  # 7-day recovery window so disabling (after state-rm of the EFS) does not
  # immediately drop the admin credential. Version stays unmanaged — no
  # aws_secretsmanager_secret_version here.
  recovery_window_in_days = 7

  tags = { Name = "${local.name}-indexer" }
}

# ── IAM ──────────────────────────────────────────────────────────────────────

resource "aws_iam_role" "indexer_execution" {
  count = var.indexer_enabled ? 1 : 0

  name               = "${local.name}-indexer-execution"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json

  tags = { Name = "${local.name}-indexer-execution" }
}

resource "aws_iam_role_policy_attachment" "indexer_execution_managed" {
  count = var.indexer_enabled ? 1 : 0

  role       = aws_iam_role.indexer_execution[0].name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "indexer_execution_secrets" {
  count = var.indexer_enabled ? 1 : 0

  name = "read-indexer-secret"
  role = aws_iam_role.indexer_execution[0].id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["secretsmanager:GetSecretValue"]
      Resource = [aws_secretsmanager_secret.indexer[0].arn]
    }]
  })
}

resource "aws_iam_role" "indexer_task" {
  count = var.indexer_enabled ? 1 : 0

  name               = "${local.name}-indexer-task"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json

  tags = { Name = "${local.name}-indexer-task" }
}

resource "aws_iam_role_policy" "indexer_task_exec_channel" {
  count = var.indexer_enabled ? 1 : 0

  name = "ecs-exec-ssm-channel"
  role = aws_iam_role.indexer_task[0].id

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

resource "aws_lb_target_group" "indexer" {
  count = var.indexer_enabled ? 1 : 0

  name        = "${local.name}-indexer"
  port        = local.indexer_port
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = aws_vpc.main.id

  # Health: GET /health on PORT (8787). Listing: GET /api/markets.
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

  tags = { Name = "${local.name}-indexer" }
}

# Priority 100 — no other listener rules exist today; the default action remains
# the relay target group. Do not steal that default.
resource "aws_lb_listener_rule" "indexer_markets_host" {
  count = var.indexer_enabled ? 1 : 0

  listener_arn = aws_lb_listener.https[0].arn
  priority     = 100

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.indexer[0].arn
  }

  condition {
    host_header {
      values = [local.markets_fqdn]
    }
  }

  tags = { Name = "${local.name}-indexer-markets-host" }
}

# ── Task definition ──────────────────────────────────────────────────────────

resource "aws_ecs_task_definition" "indexer" {
  count = var.indexer_enabled ? 1 : 0

  family                   = "${local.name}-indexer"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.indexer_cpu
  memory                   = var.indexer_memory
  execution_role_arn       = aws_iam_role.indexer_execution[0].arn
  task_role_arn            = aws_iam_role.indexer_task[0].arn

  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }

  volume {
    name = "indexer-data"

    efs_volume_configuration {
      file_system_id     = aws_efs_file_system.indexer[0].id
      transit_encryption = "ENABLED"

      authorization_config {
        access_point_id = aws_efs_access_point.indexer[0].id
        iam             = "DISABLED"
      }
    }
  }

  container_definitions = jsonencode([{
    name      = "indexer"
    image     = var.indexer_image
    essential = true

    portMappings = [
      { containerPort = local.indexer_port, protocol = "tcp" },
    ]

    mountPoints = [{
      sourceVolume  = "indexer-data"
      containerPath = local.indexer_data_path
      readOnly      = false
    }]

    environment = [
      # Confirmed against @the-situation/indexer@0.19.1:
      #   listen var is PORT (default 3000) — not INDEXER_PORT; binds 0.0.0.0
      #   required to start: ADMIN_API_KEY + VOYAGER_API_KEY|VOYAGER_API_KEYS
      #   also used: STARKNET_NETWORK, STARKNET_RPC_URL, DB_PATH, VOYAGER_API_BASE_URL
      # Optional poll knobs (EVENT/STATE/POSITION_POLL_INTERVAL_MS, VOYAGER_*)
      # are left at binary defaults.
      { name = "PORT", value = tostring(local.indexer_port) },
      { name = "STARKNET_NETWORK", value = "mainnet" },
      { name = "STARKNET_RPC_URL", value = local.indexer_starknet_rpc_url },
      { name = "VOYAGER_API_BASE_URL", value = "https://api.voyager.online/beta" },
      { name = "DB_PATH", value = local.indexer_db_path },
    ]

    # valueFrom with a trailing :key:: pulls one field out of the JSON secret.
    # Voyager is required only to *boot*; GET /api/markets after POST /admin/markets
    # is SQLite-only, so a dummy VOYAGER_API_KEY is enough until live polling.
    secrets = [
      { name = "ADMIN_API_KEY", valueFrom = "${aws_secretsmanager_secret.indexer[0].arn}:ADMIN_API_KEY::" },
      { name = "VOYAGER_API_KEY", valueFrom = "${aws_secretsmanager_secret.indexer[0].arn}:VOYAGER_API_KEY::" },
    ]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.indexer[0].name
        "awslogs-region"        = var.aws_region
        "awslogs-stream-prefix" = "indexer"
      }
    }
  }])

  tags = { Name = "${local.name}-indexer" }
}

# ── Service ──────────────────────────────────────────────────────────────────

resource "aws_ecs_service" "indexer" {
  count = var.indexer_enabled ? 1 : 0

  name            = "indexer"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.indexer[0].arn
  desired_count   = local.indexer_running ? var.indexer_desired_count : 0
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.indexer[0].id]
    assign_public_ip = true
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.indexer[0].arn
    container_name   = "indexer"
    container_port   = local.indexer_port
  }

  # One task + shared SQLite on EFS: stop-then-start rather than briefly
  # running two writers against one database file.
  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100

  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  health_check_grace_period_seconds = 120

  enable_execute_command = true

  depends_on = [
    aws_lb_listener_rule.indexer_markets_host,
    aws_iam_role_policy.indexer_execution_secrets,
    aws_iam_role_policy_attachment.indexer_execution_managed,
    aws_efs_mount_target.indexer,
  ]

  # Deliberately does NOT copy paymaster's ignore_changes = [desired_count].
  # Enable is two-phase: create at desired_count = 0, populate the secret, then
  # set desired_count = 1 and apply. Ignoring desired_count would make that
  # second apply a no-op and leave the service at zero tasks forever.

  tags = { Name = "${local.name}-indexer" }
}
