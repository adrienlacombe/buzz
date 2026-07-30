resource "aws_elasticache_subnet_group" "main" {
  name        = "${local.name}-redis"
  description = "Private subnets for ElastiCache"
  subnet_ids  = aws_subnet.private[*].id
}

# Single node, no replication and no TLS. The relay uses Redis for pub/sub
# fan-out, presence and typing indicators — all reconstructible from a cold
# cache, so losing the node costs a reconnect, not data.
#
# TLS (transit_encryption_enabled) and AUTH require aws_elasticache_replication_group
# rather than this resource. Both are worth adding at the production tier; here
# the node is unreachable outside the VPC and locked to the relay's security group.
resource "aws_elasticache_cluster" "main" {
  cluster_id = "${local.name}-redis"

  engine               = "redis"
  engine_version       = var.redis_engine_version
  node_type            = var.redis_node_type
  num_cache_nodes      = 1
  parameter_group_name = "default.redis7"
  port                 = 6379

  subnet_group_name  = aws_elasticache_subnet_group.main.name
  security_group_ids = [aws_security_group.redis.id]

  # Single-node clusters cannot fail over, so a maintenance restart is a brief
  # outage. Pinned to the same quiet window as RDS.
  maintenance_window       = "sun:04:30-sun:05:30"
  apply_immediately        = false
  snapshot_retention_limit = 0 # cache only — nothing here is worth backing up

  tags = { Name = "${local.name}-redis" }
}
