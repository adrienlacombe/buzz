output "relay_url" {
  description = "WebSocket URL clients connect to (BUZZ_RELAY_URL / desktop community relay)."
  value       = local.relay_url
}

output "relay_http_url" {
  description = "HTTP origin for NIP-11 metadata, media and git smart HTTP."
  value       = local.media_base_url
}

output "alb_dns_name" {
  description = "ALB hostname. Useful even with a domain, for testing before DNS propagates."
  value       = aws_lb.main.dns_name
}

output "tls_enabled" {
  description = "Whether a real certificate is attached. False means ws:// only — browsers on an https page will refuse to connect."
  value       = local.enable_dns
}

output "media_bucket" {
  description = "S3 bucket holding media blobs."
  value       = aws_s3_bucket.media.id
}

output "postgres_endpoint" {
  description = "RDS endpoint. Not publicly reachable — reach it from inside the VPC."
  value       = aws_db_instance.main.endpoint
}

output "redis_endpoint" {
  description = "ElastiCache primary endpoint."
  value       = "${aws_elasticache_cluster.main.cache_nodes[0].address}:${aws_elasticache_cluster.main.port}"
}

output "ecs_cluster" {
  description = "ECS cluster name."
  value       = aws_ecs_cluster.main.name
}

output "log_group" {
  description = "CloudWatch log group for relay logs."
  value       = aws_cloudwatch_log_group.relay.name
}

output "identity_secret_id" {
  description = "Secret holding BUZZ_RELAY_PRIVATE_KEY. Terraform never writes a real value here."
  value       = aws_secretsmanager_secret.identity.name
}

output "next_steps" {
  description = "Commands to finish the deploy."
  value       = <<-EOT

    1. Set the relay identity key (the service crash-loops until you do):

         aws secretsmanager put-secret-value \
           --profile ${var.aws_profile} --region ${var.aws_region} \
           --secret-id "${aws_secretsmanager_secret.identity.name}" \
           --secret-string "$(openssl rand -hex 32)"

       Then force a new deployment so the task picks it up:

         aws ecs update-service --force-new-deployment \
           --profile ${var.aws_profile} --region ${var.aws_region} \
           --cluster ${aws_ecs_cluster.main.name} --service relay

    2. Watch it come up:

         aws logs tail ${aws_cloudwatch_log_group.relay.name} --follow \
           --profile ${var.aws_profile} --region ${var.aws_region}

    3. Verify:

         curl -fsS ${local.media_base_url}/health && echo OK

    4. Point the CLI at it:

         export BUZZ_RELAY_URL=${local.relay_url}

  EOT
}
