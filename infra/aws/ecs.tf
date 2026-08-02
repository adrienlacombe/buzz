resource "aws_ecs_cluster" "main" {
  name = local.name

  setting {
    name  = "containerInsights"
    value = "disabled" # billed per metric; enable when you need the dashboards
  }

  tags = { Name = local.name }
}

resource "aws_cloudwatch_log_group" "relay" {
  name              = "/ecs/${local.name}/relay"
  retention_in_days = var.log_retention_days

  tags = { Name = "${local.name}-relay" }
}

# ── IAM ──────────────────────────────────────────────────────────────────────

data "aws_iam_policy_document" "ecs_assume_role" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole"]

    principals {
      type        = "Service"
      identifiers = ["ecs-tasks.amazonaws.com"]
    }
  }
}

# Used by the ECS agent, not the relay: pulls the image, writes logs, and reads
# the secrets it injects into the container.
resource "aws_iam_role" "execution" {
  name               = "${local.name}-ecs-execution"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json

  tags = { Name = "${local.name}-ecs-execution" }
}

resource "aws_iam_role_policy_attachment" "execution_managed" {
  role       = aws_iam_role.execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "execution_secrets" {
  name = "read-runtime-secrets"
  role = aws_iam_role.execution.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = ["secretsmanager:GetSecretValue"]
      Resource = [
        aws_secretsmanager_secret.runtime.arn,
        aws_secretsmanager_secret.identity.arn,
      ]
    }]
  })
}

# Assumed by the relay process itself. S3 goes through the scoped IAM user key
# (see s3.tf for why), so the only thing here is the SSM channel that backs
# `aws ecs execute-command`.
resource "aws_iam_role" "task" {
  name               = "${local.name}-ecs-task"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json

  tags = { Name = "${local.name}-ecs-task" }
}

# enable_execute_command on the service is not sufficient on its own — without
# these four actions on the task role, exec fails with a TargetNotConnected error
# that gives no hint about the missing permission.
resource "aws_iam_role_policy" "task_exec_channel" {
  name = "ecs-exec-ssm-channel"
  role = aws_iam_role.task.id

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

resource "aws_ecs_task_definition" "relay" {
  family                   = "${local.name}-relay"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.relay_cpu
  memory                   = var.relay_memory
  execution_role_arn       = aws_iam_role.execution.arn
  task_role_arn            = aws_iam_role.task.arn

  runtime_platform {
    operating_system_family = "LINUX"
    # The published image is multi-arch; X86_64 is the safe default. Switch to
    # ARM64 for ~20% cheaper Fargate if the tag ships a linux/arm64 manifest.
    cpu_architecture = "X86_64"
  }

  volume {
    name = "git-repos"

    efs_volume_configuration {
      file_system_id     = aws_efs_file_system.git.id
      transit_encryption = "ENABLED"

      authorization_config {
        access_point_id = aws_efs_access_point.git.id
        iam             = "DISABLED"
      }
    }
  }

  container_definitions = jsonencode([{
    name      = "relay"
    image     = var.relay_image
    essential = true

    portMappings = [
      { containerPort = local.relay_port, protocol = "tcp" },
      { containerPort = local.health_port, protocol = "tcp" },
      { containerPort = local.metrics_port, protocol = "tcp" },
    ]

    mountPoints = [{
      sourceVolume  = "git-repos"
      containerPath = local.git_repo_path
      readOnly      = false
    }]

    environment = [
      { name = "BUZZ_BIND_ADDR", value = "0.0.0.0:${local.relay_port}" },
      { name = "BUZZ_HEALTH_PORT", value = tostring(local.health_port) },
      { name = "BUZZ_METRICS_PORT", value = tostring(local.metrics_port) },

      { name = "REDIS_URL", value = "redis://${aws_elasticache_cluster.main.cache_nodes[0].address}:${aws_elasticache_cluster.main.port}" },

      { name = "BUZZ_S3_ENDPOINT", value = local.s3_endpoint },
      { name = "BUZZ_S3_BUCKET", value = aws_s3_bucket.media.id },
      { name = "BUZZ_S3_REGION", value = var.aws_region },
      { name = "BUZZ_S3_ADDRESSING_STYLE", value = local.s3_addressing_style },

      { name = "RELAY_URL", value = local.relay_url },
      { name = "BUZZ_MEDIA_BASE_URL", value = local.media_base_url },
      { name = "RELAY_OWNER_PUBKEY", value = var.owner_pubkey },

      # Restricts the relay to pubkeys in its membership table. The owner is
      # bootstrapped as a member on startup, so this does not lock the owner out.
      # Guarded by a variable validation: enabling it without owner_pubkey makes
      # the relay exit at startup.
      { name = "BUZZ_REQUIRE_RELAY_MEMBERSHIP", value = tostring(var.require_relay_membership) },

      # Serves the repo browser SPA at /repos (router.rs:225). Assets already
      # ship in the image via BUZZ_WEB_DIR, so this only enables the route.
      { name = "BUZZ_SERVE_GIT_WEB_GUI", value = tostring(var.serve_git_web_gui) },

      # Applies migrations/ on startup, so no separate migration task.
      { name = "BUZZ_AUTO_MIGRATE", value = "true" },

      { name = "BUZZ_GIT_REPO_PATH", value = local.git_repo_path },

      # BUZZ_STARKNET_RPC_SN_MAIN was injected here for NIP-SW wallet-binding
      # verification. NIP-SW was withdrawn and no relay code reads it, so it is
      # gone; the endpoint now belongs to buzz-paymaster. See paymaster.tf.

      { name = "RUST_LOG", value = var.log_level },
    ]

    # valueFrom with a trailing :key:: pulls one field out of the JSON secret.
    secrets = [
      { name = "DATABASE_URL", valueFrom = "${aws_secretsmanager_secret.runtime.arn}:DATABASE_URL::" },
      { name = "BUZZ_S3_ACCESS_KEY", valueFrom = "${aws_secretsmanager_secret.runtime.arn}:BUZZ_S3_ACCESS_KEY::" },
      { name = "BUZZ_S3_SECRET_KEY", valueFrom = "${aws_secretsmanager_secret.runtime.arn}:BUZZ_S3_SECRET_KEY::" },
      { name = "BUZZ_GIT_HOOK_HMAC_SECRET", valueFrom = "${aws_secretsmanager_secret.runtime.arn}:BUZZ_GIT_HOOK_HMAC_SECRET::" },
      # Plain string secret, not JSON — no key suffix.
      { name = "BUZZ_RELAY_PRIVATE_KEY", valueFrom = aws_secretsmanager_secret.identity.arn },
    ]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.relay.name
        "awslogs-region"        = var.aws_region
        "awslogs-stream-prefix" = "relay"
      }
    }
  }])

  tags = { Name = "${local.name}-relay" }
}

# ── Service ──────────────────────────────────────────────────────────────────

resource "aws_ecs_service" "relay" {
  name            = "relay"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.relay.arn
  desired_count   = var.relay_desired_count
  launch_type     = "FARGATE"

  # Public subnets with a public IP instead of private subnets behind a NAT
  # gateway: saves ~$32/mo, and the security group still allows ingress only
  # from the ALB. The public IP is needed to pull from ghcr.io.
  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.relay.id]
    assign_public_ip = true
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.relay.arn
    container_name   = "relay"
    container_port   = local.relay_port
  }

  # The relay applies migrations at boot and holds git state on a shared EFS
  # volume, so two tasks briefly overlapping is the risk to manage. At
  # desired_count 1 this forces stop-then-start: a short outage in exchange for
  # never running two migrating relays against one database.
  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100

  # With CI deploying on every push to main, this is the safety net that matters
  # most: if the new task definition fails to reach a steady state, ECS rolls the
  # service back to the last working one on its own. Doing this natively is
  # better than scripting a rollback in the workflow, because it also covers
  # deployments triggered outside CI.
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  # First boot pulls the image, runs migrations and opens listeners. Without the
  # grace period the ALB would mark it unhealthy and ECS would kill it mid-migration.
  health_check_grace_period_seconds = 180

  wait_for_steady_state = false

  enable_execute_command = true # `aws ecs execute-command` for a shell in the task

  depends_on = [
    aws_lb_listener.http,
    aws_iam_role_policy.execution_secrets,
    aws_iam_role_policy_attachment.execution_managed,
    # Mount targets must exist before a task can attach the volume.
    aws_efs_mount_target.git,
    # These two are load-bearing and not obvious. The task definition references
    # the *secret* ARNs, so Terraform sees no dependency on the secret *versions*
    # that actually hold the values -- and the runtime version cannot be written
    # until RDS finishes (it embeds the endpoint in DATABASE_URL). Without this,
    # the service is created first, immediately tries to place a task, and fails
    # with "ResourceNotFoundException ... staging label AWSCURRENT" for several
    # minutes until RDS comes up. Observed on the first apply of this stack.
    aws_secretsmanager_secret_version.runtime,
  ]

  lifecycle {
    ignore_changes = [desired_count] # left free for manual or autoscaled changes
  }

  tags = { Name = "${local.name}-relay" }
}
