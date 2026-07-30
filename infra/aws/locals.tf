locals {
  name = "${var.project_name}-${var.environment}"

  # Two AZs is the floor, not a choice: an ALB requires subnets in >= 2 AZs and
  # an RDS subnet group requires >= 2. "Single-AZ" here means the RDS/Redis
  # instances are single-AZ, not that the VPC is.
  azs = slice(data.aws_availability_zones.available.names, 0, 2)

  public_subnet_cidrs  = [for i in range(2) : cidrsubnet(var.vpc_cidr, 8, i)]
  private_subnet_cidrs = [for i in range(2) : cidrsubnet(var.vpc_cidr, 8, i + 10)]

  # DNS/TLS is all-or-nothing on domain_name being set.
  enable_dns = var.domain_name != ""

  relay_fqdn = local.enable_dns ? (
    var.relay_subdomain == "" ? var.domain_name : "${var.relay_subdomain}.${var.domain_name}"
  ) : null

  # Public origin clients dial. Without a domain there is no cert, so this
  # degrades to ws:// on the ALB hostname — fine for CLI testing, but browsers
  # on an https page will refuse it and the desktop app expects wss://.
  public_host   = local.enable_dns ? local.relay_fqdn : aws_lb.main.dns_name
  public_scheme = local.enable_dns ? "wss" : "ws"
  http_scheme   = local.enable_dns ? "https" : "http"

  relay_url   = "${local.public_scheme}://${local.public_host}"
  http_origin = "${local.http_scheme}://${local.public_host}"

  # BUZZ_MEDIA_BASE_URL must end with "/media" and must NOT end with a slash --
  # buzz-media rejects anything else at startup (crates/buzz-media/src/config.rs:103).
  # Matches the chart's buzz.mediaBaseUrl helper, which builds https://<host>/media.
  media_base_url = "${local.http_origin}/media"

  # Ports, from Dockerfile EXPOSE and deploy/charts/buzz/values.yaml.
  relay_port   = 3000
  health_port  = 8080
  metrics_port = 9102

  # Dockerfile creates buzz:buzz as uid/gid 1000 with home /var/lib/buzz, and
  # the image runs as USER buzz:buzz. The EFS access point must match or the
  # relay cannot write to the git path.
  container_uid = 1000
  container_gid = 1000
  git_repo_path = "/var/lib/buzz/git"

  # Real AWS S3 wants virtual-hosted addressing; "path" is the MinIO/dev default.
  s3_endpoint         = "https://s3.${var.aws_region}.amazonaws.com"
  s3_addressing_style = "virtual"
}
