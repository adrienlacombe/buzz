resource "aws_lb" "main" {
  name               = "${local.name}-alb"
  load_balancer_type = "application"
  internal           = false
  security_groups    = [aws_security_group.alb.id]
  subnets            = aws_subnet.public[*].id

  # Nostr connections are long-lived WebSockets. The ALB default is 60s, which
  # would tear down idle subscriptions constantly; 4000s is just under the
  # 4200s ceiling and lets the relay's own keepalives govern liveness instead.
  idle_timeout = 4000

  drop_invalid_header_fields = true
  enable_deletion_protection = var.deletion_protection

  tags = { Name = "${local.name}-alb" }
}

resource "aws_lb_target_group" "relay" {
  name        = "${local.name}-relay"
  port        = local.relay_port
  protocol    = "HTTP"
  target_type = "ip" # required for Fargate awsvpc networking
  vpc_id      = aws_vpc.main.id

  # Probe the dedicated health listener (main.rs:1199 binds 0.0.0.0:8080) rather
  # than the traffic port, so a saturated relay still reports honestly.
  #
  # Path must be /_readiness, NOT /health. The health router only serves
  # /_liveness, /_readiness, /_status and /_mesh (router.rs:239); /health exists
  # only on the main router on port 3000. Probing 8080/health returns 404, which
  # marks the target unhealthy -- and because an ALB fails open when *every*
  # target is unhealthy, traffic still flows and the misconfiguration hides
  # itself. /_readiness is also the semantically correct choice: it pings
  # Postgres and Redis, and returns 503 once SIGTERM starts a graceful drain, so
  # the ALB stops sending new connections to a task that is shutting down.
  health_check {
    enabled             = true
    path                = "/_readiness"
    port                = tostring(local.health_port)
    protocol            = "HTTP"
    matcher             = "200"
    interval            = 15
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
  }

  # Give in-flight WebSocket sessions time to drain on deploy instead of the
  # 300s default holding every deployment open.
  deregistration_delay = 30

  lifecycle {
    create_before_destroy = true
  }

  tags = { Name = "${local.name}-relay" }
}

# ── Listeners ────────────────────────────────────────────────────────────────

# With a domain: 80 redirects to 443. Without: 80 is the only way in, so it must
# actually serve traffic rather than redirect to a listener that does not exist.
resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.main.arn
  port              = 80
  protocol          = "HTTP"

  dynamic "default_action" {
    for_each = local.enable_dns ? [1] : []
    content {
      type = "redirect"
      redirect {
        port        = "443"
        protocol    = "HTTPS"
        status_code = "HTTP_301"
      }
    }
  }

  dynamic "default_action" {
    for_each = local.enable_dns ? [] : [1]
    content {
      type             = "forward"
      target_group_arn = aws_lb_target_group.relay.arn
    }
  }
}

resource "aws_lb_listener" "https" {
  count = local.enable_dns ? 1 : 0

  load_balancer_arn = aws_lb.main.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = aws_acm_certificate_validation.main[0].certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.relay.arn
  }
}
