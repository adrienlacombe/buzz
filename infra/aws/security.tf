# Security groups are written as standalone rules rather than inline blocks so
# the ALB <-> relay pair can reference each other without a dependency cycle.

resource "aws_security_group" "alb" {
  name        = "${local.name}-alb"
  description = "Public ingress to the Buzz ALB"
  vpc_id      = aws_vpc.main.id

  tags = { Name = "${local.name}-alb" }

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_vpc_security_group_ingress_rule" "alb_http" {
  security_group_id = aws_security_group.alb.id
  # EC2 rejects non-ASCII in security group and rule descriptions, so keep every
  # description in this file plain ASCII - no em-dashes.
  description = "HTTP - redirects to HTTPS when a domain is configured"
  cidr_ipv4   = "0.0.0.0/0"
  from_port   = 80
  to_port     = 80
  ip_protocol = "tcp"
}

resource "aws_vpc_security_group_ingress_rule" "alb_https" {
  count = local.enable_dns ? 1 : 0

  security_group_id = aws_security_group.alb.id
  description       = "HTTPS / WSS"
  cidr_ipv4         = "0.0.0.0/0"
  from_port         = 443
  to_port           = 443
  ip_protocol       = "tcp"
}

resource "aws_vpc_security_group_egress_rule" "alb_to_relay" {
  security_group_id            = aws_security_group.alb.id
  description                  = "Forward traffic to the relay"
  referenced_security_group_id = aws_security_group.relay.id
  from_port                    = local.relay_port
  to_port                      = local.relay_port
  ip_protocol                  = "tcp"
}

resource "aws_vpc_security_group_egress_rule" "alb_to_relay_health" {
  security_group_id            = aws_security_group.alb.id
  description                  = "Target group health checks on the dedicated health port"
  referenced_security_group_id = aws_security_group.relay.id
  from_port                    = local.health_port
  to_port                      = local.health_port
  ip_protocol                  = "tcp"
}

# ── Relay tasks ──────────────────────────────────────────────────────────────

resource "aws_security_group" "relay" {
  name        = "${local.name}-relay"
  description = "Buzz relay Fargate tasks"
  vpc_id      = aws_vpc.main.id

  tags = { Name = "${local.name}-relay" }

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_vpc_security_group_ingress_rule" "relay_from_alb" {
  security_group_id            = aws_security_group.relay.id
  description                  = "Relay traffic from the ALB only"
  referenced_security_group_id = aws_security_group.alb.id
  from_port                    = local.relay_port
  to_port                      = local.relay_port
  ip_protocol                  = "tcp"
}

resource "aws_vpc_security_group_ingress_rule" "relay_health_from_alb" {
  security_group_id            = aws_security_group.relay.id
  description                  = "Health probes from the ALB"
  referenced_security_group_id = aws_security_group.alb.id
  from_port                    = local.health_port
  to_port                      = local.health_port
  ip_protocol                  = "tcp"
}

# Unrestricted egress: the task pulls its image from ghcr.io, reads secrets from
# Secrets Manager, and talks to S3/RDS/Redis/EFS. Narrowing this needs VPC
# endpoints for ECR/Secrets Manager/CloudWatch, which the dev tier skips.
resource "aws_vpc_security_group_egress_rule" "relay_all" {
  security_group_id = aws_security_group.relay.id
  description       = "Image pull, AWS APIs, backing services"
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "-1"
}

# ── Backing services ─────────────────────────────────────────────────────────

resource "aws_security_group" "postgres" {
  name        = "${local.name}-postgres"
  description = "RDS Postgres - relay tasks only"
  vpc_id      = aws_vpc.main.id

  tags = { Name = "${local.name}-postgres" }
}

resource "aws_vpc_security_group_ingress_rule" "postgres_from_relay" {
  security_group_id            = aws_security_group.postgres.id
  referenced_security_group_id = aws_security_group.relay.id
  from_port                    = 5432
  to_port                      = 5432
  ip_protocol                  = "tcp"
}

resource "aws_security_group" "redis" {
  name        = "${local.name}-redis"
  description = "ElastiCache Redis - relay tasks only"
  vpc_id      = aws_vpc.main.id

  tags = { Name = "${local.name}-redis" }
}

resource "aws_vpc_security_group_ingress_rule" "redis_from_relay" {
  security_group_id            = aws_security_group.redis.id
  referenced_security_group_id = aws_security_group.relay.id
  from_port                    = 6379
  to_port                      = 6379
  ip_protocol                  = "tcp"
}

resource "aws_security_group" "efs" {
  name        = "${local.name}-efs"
  description = "EFS git storage - relay tasks only"
  vpc_id      = aws_vpc.main.id

  tags = { Name = "${local.name}-efs" }
}

resource "aws_vpc_security_group_ingress_rule" "efs_from_relay" {
  security_group_id            = aws_security_group.efs.id
  description                  = "NFS"
  referenced_security_group_id = aws_security_group.relay.id
  from_port                    = 2049
  to_port                      = 2049
  ip_protocol                  = "tcp"
}
