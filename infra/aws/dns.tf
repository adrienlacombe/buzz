# Everything here is created only when var.domain_name is set. Register the
# domain in the Route 53 console first — that keeps the WHOIS contact details
# (legal name, address, phone) out of Terraform state, which is plaintext.
#
# Registration also auto-creates the hosted zone this data source reads.

data "aws_route53_zone" "main" {
  count = local.enable_dns ? 1 : 0

  name         = var.domain_name
  private_zone = false
}

# ACM certificates for an ALB must live in the ALB's own region (eu-west-3).
# The us-east-1 requirement people remember applies to CloudFront, not ALB.
resource "aws_acm_certificate" "main" {
  count = local.enable_dns ? 1 : 0

  domain_name       = local.relay_fqdn
  validation_method = "DNS"

  tags = { Name = local.relay_fqdn }

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_route53_record" "cert_validation" {
  for_each = local.enable_dns ? {
    for dvo in aws_acm_certificate.main[0].domain_validation_options :
    dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  } : {}

  zone_id         = data.aws_route53_zone.main[0].zone_id
  name            = each.value.name
  type            = each.value.type
  records         = [each.value.record]
  ttl             = 60
  allow_overwrite = true
}

# Blocks until ACM observes the validation records, so the HTTPS listener is
# never created against a still-pending certificate.
resource "aws_acm_certificate_validation" "main" {
  count = local.enable_dns ? 1 : 0

  certificate_arn         = aws_acm_certificate.main[0].arn
  validation_record_fqdns = [for r in aws_route53_record.cert_validation : r.fqdn]

  timeouts {
    create = "10m"
  }
}

# Alias record, not CNAME: an alias resolves at the zone apex too, and AWS does
# not bill queries against it.
resource "aws_route53_record" "relay" {
  count = local.enable_dns ? 1 : 0

  zone_id = data.aws_route53_zone.main[0].zone_id
  name    = local.relay_fqdn
  type    = "A"

  alias {
    name                   = aws_lb.main.dns_name
    zone_id                = aws_lb.main.zone_id
    evaluate_target_health = true
  }
}
