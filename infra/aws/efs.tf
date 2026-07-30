# Git repositories the relay hosts are mutable state on a filesystem path
# (BUZZ_GIT_REPO_PATH), not objects in S3, so they need a real volume. EFS is the
# only Fargate-compatible option — EBS cannot attach to a Fargate task.

resource "aws_efs_file_system" "git" {
  creation_token = "${local.name}-git"
  encrypted      = true

  # Bursting suits a git workload: mostly idle, with spikes on clone/push.
  throughput_mode = "bursting"

  lifecycle_policy {
    transition_to_ia = "AFTER_30_DAYS"
  }

  tags = { Name = "${local.name}-git" }

  lifecycle {
    # Holds every hosted git repository. Unattended CI must never be able to
    # replace this. See rds.tf for how to tear down deliberately.
    prevent_destroy = true
  }
}

resource "aws_efs_mount_target" "git" {
  count = 2

  file_system_id  = aws_efs_file_system.git.id
  subnet_id       = aws_subnet.private[count.index].id
  security_groups = [aws_security_group.efs.id]
}

# The image runs as USER buzz:buzz = uid/gid 1000 (Dockerfile:142,160). An EFS
# root is owned by root:root, so without an access point pinning ownership the
# relay would fail to create repositories with EACCES.
resource "aws_efs_access_point" "git" {
  file_system_id = aws_efs_file_system.git.id

  posix_user {
    uid = local.container_uid
    gid = local.container_gid
  }

  root_directory {
    path = "/git"

    creation_info {
      owner_uid   = local.container_uid
      owner_gid   = local.container_gid
      permissions = "0755"
    }
  }

  tags = { Name = "${local.name}-git" }
}
