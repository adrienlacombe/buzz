# Buzz relay on AWS (ECS Fargate)

**FORK-LOCAL** — not present in `block/buzz`. Upstream deploys via
`deploy/charts/buzz` (Helm/Kubernetes). This is an independent Terraform path for
`adrienlacombe/buzz` targeting AWS account `618867225791` in `eu-west-3`. It adds
only new files under a new directory, so an upstream sync should never conflict here.

## Architecture

```
                     Route 53  (optional — only when domain_name is set)
                         │
                    ┌────▼─────┐
   clients ─────────►   ALB    │  :443 wss  (:80 → 301, or :80 http when no domain)
   wss://relay.…    └────┬─────┘  idle_timeout 4000s for long-lived WebSockets
                         │
                  public subnets ×2 (AZ a, b)
                    ┌────▼──────────────┐
                    │ ECS Fargate task  │  ghcr.io/block/buzz
                    │  :3000  relay     │  public IP, no NAT gateway
                    │  :8080  /health   │  SG allows ingress from ALB only
                    │  :9102  metrics   │
                    └────┬──────────────┘
                         │  private subnets ×2 — no internet route
      ┌──────────────────┼──────────────────┬─────────────────┐
      ▼                  ▼                  ▼                 ▼
  RDS Postgres     ElastiCache Redis      EFS              S3 (media)
  17.10            7.1, 1 node         git repos        via VPC gateway
  db.t4g.micro     cache.t4g.micro     uid/gid 1000     endpoint
  single-AZ        no TLS/AUTH         access point
```

Two AZs is a floor, not a preference: an ALB requires subnets in ≥2 AZs and an
RDS subnet group requires ≥2. "Single-AZ" refers to the RDS and Redis instances.

## Files

| File | Contents |
|---|---|
| `versions.tf` | Provider constraints, S3 backend |
| `providers.tf` | Provider config, default tags, AZ/identity data sources |
| `variables.tf` | All inputs |
| `locals.tf` | Name prefix, CIDR math, ports, derived URLs |
| `network.tf` | VPC, subnets, IGW, route tables, S3 gateway endpoint |
| `security.tf` | Security groups as standalone rules (avoids ALB↔relay cycle) |
| `rds.tf` | Postgres + password |
| `redis.tf` | ElastiCache |
| `s3.tf` | Media bucket + the relay's scoped IAM user |
| `efs.tf` | Git volume + access point |
| `secrets.tf` | Secrets Manager: `runtime` and `relay-identity` |
| `alb.tf` | ALB, target group, listeners |
| `dns.tf` | ACM cert, validation records, alias record (conditional) |
| `ecs.tf` | Cluster, IAM roles, task definition, service |
| `oidc.tf` | GitHub Actions deploy role (OIDC, no stored keys) |
| `outputs.tf` | URLs, endpoints, next-step commands |
| `dev.tfvars` | **Committed** config for the dev environment (no secrets) |
| `bootstrap/` | Creates the S3 state bucket (own local state) |

## Deploy

```bash
# 0. Credentials — the scoped IAM user, never root.
export AWS_PROFILE=alc-tf            # the S3 backend reads this
aws sts get-caller-identity
#   → arn:aws:iam::618867225791:user/terraform-buzz

# 1. State bucket (once per account).
cd bootstrap && terraform init && terraform apply && cd ..

# 2. Deploy. dev.tfvars is committed and canonical — edit it in place rather
#    than copying it, so CI and local applies stay in agreement.
#
#    relay_image is REQUIRED and has no default: CD owns which build runs, and a
#    default here meant every local apply silently reverted CD's deploy. Pass the
#    image explicitly. To keep whatever is currently deployed:
terraform init
IMAGE=$(aws ecs describe-task-definition --task-definition buzz-dev-relay \
  --region eu-west-3 --query 'taskDefinition.containerDefinitions[0].image' --output text)

terraform plan -var-file=dev.tfvars -var relay_image="$IMAGE" -out plan.tfplan   # read it
terraform apply plan.tfplan

# 4. Set the relay identity key. The service crash-loops until you do —
#    Terraform deliberately never holds this value (see secrets.tf).
aws secretsmanager put-secret-value \
  --profile alc-tf --region eu-west-3 \
  --secret-id "buzz-dev/relay-identity" \
  --secret-string "$(openssl rand -hex 32)"

aws ecs update-service --force-new-deployment \
  --profile alc-tf --region eu-west-3 \
  --cluster buzz-dev --service relay

# 5. Watch and verify.
aws logs tail /ecs/buzz-dev/relay --follow --profile alc-tf --region eu-west-3
curl -fsS "$(terraform output -raw relay_http_url)/health" && echo OK
```

## Adding the domain later

`domain_name = ""` gives you a working HTTP-only relay. That is enough for
`buzz-cli` but **not** for browsers on an HTTPS page or the desktop app, both of
which require `wss://`.

To add TLS:

1. Register the domain in the **Route 53 console** — deliberately not via
   Terraform. `aws_route53domains_domain` works, but registration requires WHOIS
   contact details (legal name, street address, phone), and every attribute
   Terraform manages is written to state in **plaintext**. Registering in the
   console keeps that PII between you and AWS.
   Also: `terraform destroy` cannot un-register a domain — AWS domains can only
   be left to expire, so a destroy/recreate cycle would silently re-buy one.
   Route 53 prices as of this writing: `.com`/`.org` $16/yr, `.net`/`.dev` $17,
   `.xyz` $19, `.app` $20, `.io` $71.
2. Registration auto-creates the hosted zone (~$0.50/mo).
3. Set `domain_name` in `terraform.tfvars` and re-apply. The certificate,
   validation records, HTTPS listener and alias record are all additive.

## Cost (dev tier, eu-west-3, rough)

| | /mo |
|---|---|
| ALB | ~$18 + LCU |
| Fargate 0.5 vCPU / 1 GB | ~$18 |
| RDS `db.t4g.micro` + 20 GB gp3 | ~$15 |
| ElastiCache `cache.t4g.micro` | ~$12 |
| EFS, S3, Secrets Manager, logs | ~$3–6 |
| **Total** | **~$65–75** |

No NAT gateway, which would add ~$32/mo on its own. Tear down with
`terraform destroy` when idle — the ALB and RDS are the expensive parts.

## Decisions worth knowing before changing them

**S3 uses a static IAM user key, not the ECS task role.** This looks like a
downgrade and is not. `buzz-media` does support the AWS credential chain
(`crates/buzz-media/src/storage.rs:29`) and the vendored `aws-creds` fork does
read `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`, so a task role resolves fine at
startup. But `rust-s3` refreshes credentials only when the caller invokes
`Bucket::credentials_refresh()`, and the relay never does — `MediaStorage` is
built once (`crates/buzz-relay/src/main.rs:446`) and held as `Arc<MediaStorage>`
for the process lifetime. Task-role credentials expire roughly every 6 hours,
after which every media operation would 403 until the task restarted. An IAM user
key does not expire. Blast radius is one bucket. This also matches what
`deploy/charts/buzz` does upstream (`S3_ACCESS_KEY` / `S3_SECRET_KEY`).

*This is arguably an upstream bug* — the documented IRSA / EKS Pod Identity
support has the same latent expiry problem on Kubernetes. Worth reporting to
`block/buzz`.

**Tasks run in public subnets.** With a public IP and no NAT gateway, saving
~$32/mo. Ingress is restricted to the ALB's security group; the public IP exists
only for egress (image pull from ghcr.io, Secrets Manager, CloudWatch).

**`desired_count = 1` with `minimum_healthy_percent = 0`.** Deploys are
stop-then-start, so there is a short outage. That is deliberate: the relay applies
migrations at boot (`BUZZ_AUTO_MIGRATE`) and git repositories are shared mutable
state on one EFS volume, so two overlapping tasks are riskier than a few seconds
of downtime. Verify the relay tolerates concurrent replicas before raising this.

**ALB `idle_timeout = 4000s`.** Nostr connections are long-lived WebSockets; the
60s default would tear down idle subscriptions constantly.

**Redis has no TLS or AUTH.** `aws_elasticache_cluster` cannot do either —
that needs `aws_elasticache_replication_group`. The node is unreachable outside
the VPC and locked to the relay's security group. Worth changing for production;
the relay uses Redis only for pub/sub, presence and typing indicators, all of
which survive a cold cache.

**State contains secrets.** The RDS password and the relay's S3 secret key are in
state in plaintext — unavoidable for those resource types. Hence the private,
versioned, encrypted bucket with a TLS-only policy, and `.gitignore` covering
`*.tfstate` and `*.tfplan`.

## Continuous deployment

`.github/workflows/deploy-aws.yml` deploys every commit that lands on `main`,
including `[upstream-sync]` merges.

```
push to main
  └─► docker.yml            publishes ghcr.io/adrienlacombe/buzz:sha-<7>
        └─► deploy-aws.yml
              ├─ verify the sha-<7> manifest exists in GHCR
              ├─ assume buzz-dev-github-actions via OIDC
              ├─ terraform apply -var-file=dev.tfvars -var relay_image=…:sha-<7>
              ├─ aws ecs wait services-stable
              └─ assert the running image matches, then curl /health
```

Details that are load-bearing:

**It triggers on `workflow_run`, not `push`.** The task definition must reference
an image tag that exists, so the deploy runs after `docker.yml` publishes rather
than racing it.

**It does not gate on `workflow_run.conclusion == 'success'`.** `docker.yml`
builds the relay *and* the push gateway. A push-gateway failure marks the whole
run `failure` even when the relay image published perfectly — observed on run
`30544720865`, where both relay manifests succeeded and only push-gateway amd64
failed. Gating on the overall conclusion would block good relay deploys on an
unrelated job, so the workflow checks for the relay manifest in GHCR instead and
no-ops cleanly when it is absent.

**Immutable tags.** Deploys pin `:sha-<7>`, never `:main`, so the running task
definition records exactly which commit is live and a rollback is
`workflow_dispatch` with an older SHA.

**No credentials in GitHub.** `oidc.tf` creates `buzz-dev-github-actions`,
assumable only by `repo:adrienlacombe/buzz:ref:refs/heads/main` — a PR branch or
a fork cannot assume it. Actions exchanges its OIDC token for short-lived STS
credentials; there is no access key to leak or rotate. The role gets
`PowerUserAccess` plus an IAM policy scoped by ARN to `buzz-*` names (not
`IAMFullAccess`), and is explicitly **denied** `secretsmanager:GetSecretValue` on
the relay identity secret — which is why `secrets.tf` deliberately does not
manage that secret's version.

**Rollback is ECS-native.** `deployment_circuit_breaker { rollback = true }` in
`ecs.tf` reverts a task definition that never stabilises, covering deploys
triggered outside CI too. The workflow then asserts the running image equals the
deployed image, so a rolled-back deploy reports red instead of passing quietly.

**Unattended applies cannot delete data.** RDS, EFS and the media bucket carry
`prevent_destroy`, so Terraform refuses to plan a replacement rather than
silently recreating them. This also blocks `terraform destroy` — to tear down
deliberately, comment out those three `lifecycle` blocks, apply, then destroy.

## Restricting who and what can reach this relay

Two independent controls, in opposite directions. Both are needed; neither
substitutes for the other.

**Where our clients may go — client-side host allowlist.** The desktop and mobile
apps ship locked to `relay.bitcoinmarkets.app`
(`desktop/src-tauri/src/relay_allowlist.rs`,
`mobile/lib/shared/relay/relay_allowlist.dart`). Enforced at each app's WebSocket
transport, which every session must pass through, so the community switcher, deep
links, invites, `BUZZ_RELAY_URL` and stored communities are all covered. Loopback
still works in debug builds, or local development and every E2E test would break.

This is a **configuration lock, not a security boundary.** It stops the shipped
app from talking to another relay. It cannot stop someone who rebuilds the client
or points `buzz-cli` at a different relay.

**Who may use our relay — `require_relay_membership`.** Set it `true` in
`dev.tfvars` and only pubkeys in the relay's membership table may use the relay;
NIP-42 authentication alone is not enough. The owner is bootstrapped as a member
at startup, so enabling it does not lock you out.

It requires `owner_pubkey`. buzz-relay *exits at startup* when membership is
required and no owner pubkey is set (`crates/buzz-relay/src/main.rs:228`), so a
variable validation rejects that combination at plan time — otherwise the apply
succeeds and the service crash-loops. A second validation rejects an
`owner_pubkey` that is not 64 lowercase hex characters, because the relay only
warns and ignores a malformed one.

```hcl
owner_pubkey             = "<your 64-hex Nostr pubkey>"   # npub must be converted
require_relay_membership = true
```

## Not covered

- **Production hardening** — `db_multi_az`, Redis replication + TLS, private
  subnets with a NAT gateway, `deletion_protection`, longer backup retention.
  The variables exist; flip them and re-apply.
- **Autoscaling** — `desired_count` is under `ignore_changes`, so an
  `aws_appautoscaling_target` can be added without fighting Terraform.
- **Staging environment** — one environment (`dev`). A second would want its own
  tfvars, state key and OIDC role.
- **Alarms** — no CloudWatch alarms or SNS topics.
- **The pairing relay** (`buzz-pair-relay`) and **push gateway**, both of which
  the Helm chart can deploy.
