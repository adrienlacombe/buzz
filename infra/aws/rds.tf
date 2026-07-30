resource "aws_db_subnet_group" "main" {
  name        = "${local.name}-postgres"
  description = "Private subnets for RDS"
  subnet_ids  = aws_subnet.private[*].id

  tags = { Name = "${local.name}-postgres" }
}

# Alphanumeric only, deliberately: this password is interpolated into a
# postgres:// URL, and punctuation there would need percent-encoding. 40 chars
# of [A-Za-z0-9] is far more entropy than the shorter mixed-charset default.
resource "random_password" "db" {
  length  = 40
  special = false
}

resource "aws_db_instance" "main" {
  identifier = "${local.name}-postgres"

  engine         = "postgres"
  engine_version = var.db_engine_version
  instance_class = var.db_instance_class

  db_name  = "buzz"
  username = "buzz"
  password = random_password.db.result
  port     = 5432

  allocated_storage     = var.db_allocated_storage
  max_allocated_storage = var.db_max_allocated_storage
  storage_type          = "gp3"
  storage_encrypted     = true

  db_subnet_group_name   = aws_db_subnet_group.main.name
  vpc_security_group_ids = [aws_security_group.postgres.id]
  publicly_accessible    = false
  multi_az               = var.db_multi_az

  backup_retention_period = var.db_backup_retention_days
  backup_window           = "02:00-03:00"
  maintenance_window      = "sun:03:30-sun:04:30"
  copy_tags_to_snapshot   = true

  # Minor versions carry security fixes and are backwards compatible; majors are
  # pinned via db_engine_version so an upgrade is always a deliberate change.
  auto_minor_version_upgrade = true

  performance_insights_enabled = false
  deletion_protection          = var.deletion_protection
  skip_final_snapshot          = var.skip_final_snapshot
  final_snapshot_identifier    = var.skip_final_snapshot ? null : "${local.name}-postgres-final"

  # The relay applies migrations/ at boot via BUZZ_AUTO_MIGRATE, so there is no
  # separate migration task to sequence before the service starts.

  tags = { Name = "${local.name}-postgres" }

  lifecycle {
    # CI applies Terraform unattended on every push to main. Without this, a
    # change to an immutable attribute (identifier, db_name, username) would
    # silently destroy and recreate the database, losing every event. Terraform
    # refuses to plan such a change instead.
    #
    # This also blocks `terraform destroy`. To tear the stack down deliberately,
    # comment out this block, apply, then destroy.
    prevent_destroy = true
  }
}
