resource "random_id" "bucket_suffix" {
  byte_length = 4
}

resource "aws_s3_bucket" "media" {
  bucket        = "${local.name}-media-${random_id.bucket_suffix.hex}"
  force_destroy = var.force_destroy_media_bucket

  tags = { Name = "${local.name}-media" }

  lifecycle {
    # Holds every uploaded media blob. Unattended CI must never be able to
    # replace this. See rds.tf for how to tear down deliberately.
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_public_access_block" "media" {
  bucket = aws_s3_bucket.media.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "media" {
  bucket = aws_s3_bucket.media.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
    bucket_key_enabled = true
  }
}

resource "aws_s3_bucket_versioning" "media" {
  bucket = aws_s3_bucket.media.id

  versioning_configuration {
    # Media blobs are content-addressed (Blossom), so a given key never changes
    # in place — versioning would only accumulate cost with nothing to recover.
    status = "Disabled"
  }
}

# Blobs are served through the relay, not read directly by browsers, so no CORS
# rule is needed. Add one only if you later point clients straight at S3.

# ── Relay S3 credentials ─────────────────────────────────────────────────────
#
# Static access keys, not the ECS task role — deliberately, and it is worth
# knowing why before "fixing" this.
#
# buzz-media supports the AWS credential chain (crates/buzz-media/src/storage.rs:29)
# and the vendored aws-creds fork does read AWS_CONTAINER_CREDENTIALS_RELATIVE_URI,
# so a task role resolves correctly at startup. But rust-s3 only refreshes
# credentials when the caller invokes Bucket::credentials_refresh(), and the relay
# never does: MediaStorage is built once (crates/buzz-relay/src/main.rs:446) and
# held as Arc<MediaStorage> for the process lifetime. Task-role credentials expire
# roughly every 6 hours, after which every media operation would 403 until the
# task restarted.
#
# An IAM user key does not expire, so it is the only option that stays working.
# The blast radius is one bucket: the policy below grants nothing else. This also
# matches what deploy/charts/buzz does upstream (S3_ACCESS_KEY / S3_SECRET_KEY).
#
# Worth reporting upstream — the documented IRSA / Pod Identity support has the
# same latent expiry bug on EKS.

resource "aws_iam_user" "relay_s3" {
  name = "${local.name}-relay-s3"
  path = "/service/"

  tags = { Name = "${local.name}-relay-s3" }
}

resource "aws_iam_user_policy" "relay_s3" {
  name = "media-bucket-access"
  user = aws_iam_user.relay_s3.name

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "ObjectAccess"
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:GetObjectTagging",
          "s3:PutObjectTagging",
        ]
        Resource = "${aws_s3_bucket.media.arn}/*"
      },
      {
        Sid      = "BucketListAndLocate"
        Effect   = "Allow"
        Action   = ["s3:ListBucket", "s3:GetBucketLocation"]
        Resource = aws_s3_bucket.media.arn
      },
    ]
  })
}

# The secret lands in Terraform state in plaintext — unavoidable for this
# resource, and the reason the state bucket is private, versioned and encrypted.
resource "aws_iam_access_key" "relay_s3" {
  user = aws_iam_user.relay_s3.name
}
