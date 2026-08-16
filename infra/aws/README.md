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
| `alb.tf` | ALB, relay target group, listeners (default action = relay) |
| `dns.tf` | ACM cert (relay + markets + paymaster SANs), validation records, alias records |
| `ecs.tf` | Cluster, IAM roles, task definition, service |
| `paymaster.tf` | **Whole** `buzz-paymaster` service — its own SG, IAM roles and secret. Off by default. Egress-only Nostr/STRK sponsor — **not** the AVNU proxy |
| `ecr.tf` | Markets indexer ECR repository (`buzz-dev-indexer`) — always created; no PassRole surface |
| `indexer.tf` | **Whole** markets indexer (`@the-situation/indexer`) — own SG, IAM, secret, EFS, TG, host-header rule. Off by default |
| `avnu-proxy.tf` | **Whole** `buzz-avnu-proxy` — public HTTPS at `paymaster.<domain>`, ALB host-header, own SG/IAM/secret. Uses `var.relay_image`. Off by default |
| `oidc.tf` | GitHub Actions deploy role + indexer ECR push role (OIDC) — lives under `bootstrap/` |
| `outputs.tf` | URLs, endpoints, next-step commands |
| `dev.tfvars` | **Committed** config for the dev environment (no secrets) |
| `bootstrap/` | Creates the S3 state bucket + OIDC deploy role + indexer ECR push role (own local state) |

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

Enabling the paymaster adds a second Fargate task at 0.25 vCPU / 0.5 GB, ~$9/mo.
Its *real* cost is on chain, not here: every sponsored transaction pays ~0.78 STRK
of BIP-340 verification, and that does not amortise. See
[`contracts/DEPLOYMENTS.md`](../../contracts/DEPLOYMENTS.md).

Enabling the markets indexer adds another Fargate task (same size by default) plus
its own EFS volume for SQLite — roughly another ~$10–12/mo on the Fargate side,
and no shared Postgres.

## Sponsorship (`buzz-paymaster`)

Off by default and safe to ignore until you want it. `paymaster_enabled = false`
means **no resources at all**, not "resources with zero tasks".

That distinction is load-bearing, and it is here because the first version got it
wrong. It created the IAM roles, secret and task definition unconditionally and only
held `desired_count` at 0 — so the next CI deploy failed with `iam:PassRole` denied on
`buzz-dev-paymaster-task`, because the bootstrap stack that grants it had not been
applied. A **relay** deploy was blocked by an optional service that was supposed to be
off. Adding an optional service must never be able to do that, so every resource in
`paymaster.tf` is gated on `count`.

Two properties are load-bearing, both explained at the top of `paymaster.tf`:

- **It listens on no port.** The paymaster connects *out* to the relay as a Nostr
  client and subscribes to kind:30900. No target group, and its security group has
  no ingress rule at all — a funded service with nothing listening is a much smaller
  target than one exposing an authenticated API. Do not add a health-check port.
- **One task, ever.** Two would collide on the Starknet account nonce and both
  service a request that arrived before either published its result — paying twice.
  A variable validation refuses any count above 1.

It runs the **same image as the relay** with the entrypoint overridden, so there is
one publish pipeline and one immutable `:sha-<7>` tag for CD to pin. See the
`FORK-LOCAL` note in the root `Dockerfile`.

### Turning it on

In this order. Step 1 before step 4, or you reproduce the failure above.

1. **Apply the bootstrap stack.** Separate state, so CI never applies it:

   ```bash
   terraform -chdir=infra/aws/bootstrap apply
   ```

   `oidc.tf` gained the paymaster's two role ARNs under `PassExecutionAndTaskRoles`;
   without them CI cannot register the task definition. The same change extends the
   `GetSecretValue` Deny to the sponsor's key, which is what stops a workflow that
   assumes the deploy role from reading a credential that can spend money.
2. **Deploy one `NostrAccount` and confirm it lands** at the address
   `buzz wallet address` derives. This verifies the UDC `deploy_from_zero`
   assumption *before* the sponsor starts deploying accounts for other people; get
   it wrong and every account lands where nobody can find it, holding whatever was
   sent to the address the user was shown.
3. **Put a funded account in the secret.** Terraform never manages its version — see
   the `put-secret-value` command in `paymaster.tf`.
4. Set `paymaster_enabled = true`, `paymaster_account_class_hash`, and
   `paymaster_desired_count = 1`.

A `precondition` fails the plan if `paymaster_enabled` is set without a class hash,
rather than letting the task start and exit non-zero where only CloudWatch would show
it.

`BUZZ_PAYMASTER_MAX_FEE_FRI` caps what a single request can cost (default 10 STRK).
It is a spending guard, not a per-member quota — there deliberately is no quota, and
what stands in its place is that the sponsor estimates every transaction before
sending, so anything that would revert is refused for free.

## Markets indexer (`@the-situation/indexer`)

Off by default and safe to ignore until you want it. `indexer_enabled = false`
means **no ECS/IAM/EFS/secret resources on the create path**, same count-gating
lesson as paymaster. The ECR repository (`buzz-dev-indexer` in `ecr.tf`) is the
exception: it is always created so the-situation-sdk can push before ECS exists.
Do not use or collide with `bim-indexer` in this account (different product).

Once enabled, setting `indexer_enabled = false` will **fail plan** because the
indexer EFS carries `lifecycle.prevent_destroy`. That is intentional so CD cannot
wipe the markets SQLite. To disable after enable you must first
`terraform state rm 'aws_efs_file_system.indexer[0]'` (and accept losing the
count-gated resources). "Off means no resources" is not a silent destroy of an
existing DB. The indexer secret uses a 7-day recovery window for the same reason.

This is **not** the relay. The container is `@the-situation/indexer` (npm);
`indexer_image` is a separate variable and must not reuse `relay_image`. Mutable
`:main` / `:latest` tags are rejected at plan time. Empty `indexer_image` is
allowed while the service is off so relay CD (which only passes `relay_image`)
keeps working; enabling without an image fails a precondition.

**Image path is Amazon ECR + IAM only.** Anonymous GHCR pull of the private
`the-situation-sdk/indexer` package is 401. Fine-grained PATs cannot do GitHub
Packages; we will not mint a classic PAT, will not make the-situation-sdk public,
and will not add `buzz-dev/indexer-ghcr` or ECS `repositoryCredentials`. The SDK
workflow on `main` assumes
`arn:aws:iam::618867225791:role/buzz-dev-indexer-ecr-push` and pushes to ECR.
The indexer execution role already has `AmazonECSTaskExecutionRolePolicy`, which
is enough to pull from ECR. Enable waits on an ECR pin of the form:

```text
618867225791.dkr.ecr.eu-west-3.amazonaws.com/buzz-dev-indexer:<immutable>@sha256:<digest>
```

Historical GHCR digest (not an enable pin):
`ghcr.io/adrienlacombe/the-situation-sdk/indexer:0.19.1@sha256:c41cf55281c2060e306d05feb108b1867473edf4dac11a223251b2fc5e0bc596`.

It is an HTTP service on port **8787**, unlike paymaster (egress-only):

- Own security group: ingress from the ALB on 8787 only
- Own EFS filesystem + access point for SQLite (`DB_PATH` under
  `/var/lib/situation-indexer`) — not the relay git EFS, not RDS
- Own Secrets Manager secret (`buzz-dev/indexer`) for `ADMIN_API_KEY` and
  `VOYAGER_API_KEY` — unmanaged version, populated out of band (does not exist
  until first enable; do not invent an indexer-ghcr secret)
- ALB HTTPS listener rule: host-header `markets.bitcoinmarkets.app` → indexer TG
- Health check `GET /health` on the traffic port — not the relay `/_readiness`
- Default listener action stays the relay

The shared ACM certificate carries a SAN for `markets.bitcoinmarkets.app` even
while the service is off (so enabling does not wait on a cert replacement). The
Route53 A alias and listener rule appear only when `indexer_enabled` is true.

Public URL for Wallet: **`INDEXER_URL=https://markets.bitcoinmarkets.app`**.

### Turning it on

CEO applies AWS with `--profile alc` when ready (this repo does not apply).
Order matters — bootstrap first. Keep `indexer_enabled = false` until the image
is actually in ECR.

1. **Apply the bootstrap stack** (separate state; CI never applies it):

   ```bash
   terraform -chdir=infra/aws/bootstrap apply
   ```

   Creates `buzz-dev-indexer-ecr-push` (OIDC trust:
   `repo:adrienlacombe/the-situation-sdk:ref:refs/heads/main` only), grants
   `iam:PassRole` on `buzz-dev-indexer-execution` / `buzz-dev-indexer-task`, and
   denies the deploy role `GetSecretValue` on `buzz-dev/indexer`. The buzz
   deploy role is not assumable by the-situation-sdk.
2. **Main stack creates ECR** `buzz-dev-indexer` on apply even while the
   indexer is disabled (CD after merge, or a local apply). Then the SDK
   workflow on `main` pushes, assuming
   `arn:aws:iam::618867225791:role/buzz-dev-indexer-ecr-push`. Copy the
   resulting immutable tag + digest.
3. **Enable resources with `desired_count = 0`** so the secret and roles exist
   without a crash-looping task:

   ```hcl
   indexer_enabled       = true
   indexer_desired_count = 0
   indexer_image         = "618867225791.dkr.ecr.eu-west-3.amazonaws.com/buzz-dev-indexer:<immutable>@sha256:<digest>"
   ```

   Apply the main stack with the usual `relay_image` pin. Do not leave a GHCR
   URI here.
4. **Populate the unmanaged secret** (keys only here — never commit values):

   ```bash
   aws secretsmanager put-secret-value \
     --profile alc --region eu-west-3 \
     --secret-id "buzz-dev/indexer" \
     --secret-string '{"ADMIN_API_KEY":"<offline>","VOYAGER_API_KEY":"<voyager>"}'
   ```

   `VOYAGER_API_KEY` (or `VOYAGER_API_KEYS`) is required to *start* in 0.19.1.
   A dummy is enough for `GET /api/markets` after an admin POST — that route is
   SQLite only. A real Voyager key is needed for event poll.
5. Set `indexer_desired_count = 1` and apply again. The indexer ECS service does
   **not** use `ignore_changes = [desired_count]` (unlike paymaster), so this
   apply actually scales the service to one task.
6. **After the task is healthy — register the v1 market** (operator step, not
   automated in Terraform; not part of enabling this PR):

   ```bash
   curl -X POST https://markets.bitcoinmarkets.app/admin/markets \
     -H "Authorization: Bearer $ADMIN_API_KEY" \
     -H "Content-Type: application/json" \
     -d '{
       "address": "0x023b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8",
       "title": "Bitcoin difficulty after next retarget",
       "description": "Collateral: BTC",
       "market_type": "lognormal",
       "x_axis_label": "Difficulty",
       "category": "bitcoin"
     }'
   ```

   Listing copy says BTC. Do not put Starknet / Cairo / STRK in the title or
   description. This is `markets.bitcoinmarkets.app`, not an sslip.io host.

7. Point Wallet at `INDEXER_URL=https://markets.bitcoinmarkets.app`.

## AVNU proxy (`buzz-avnu-proxy` at `paymaster.bitcoinmarkets.app`)

Off by default and safe to ignore until you want it. `avnu_proxy_enabled = false`
means **no ECS/IAM/secret/TG resources on the create path**, same count-gating
lesson as paymaster/indexer.

**This is not `paymaster.tf`.** That file is the old Nostr/STRK sponsor
(`buzz-paymaster`): egress-only, no inbound, wrong product for AVNU sponsored
transactions. Leave `paymaster_enabled = false`. The hostname
`paymaster.bitcoinmarkets.app` is product naming for this HTTP proxy.

Why it exists: AVNU `paymaster_buildTransaction` already succeeded via a
loopback proxy, but shipped clients refuse `AVNU_PROXY_URL` loopback
(`crates/buzz-core/src/markets.rs` `resolve_avnu_proxy_url`). Host a public
proxy so the client can call it; `AVNU_API_KEY` stays server-side. Never bake
the key into the image, repo, or client.

**Image is `var.relay_image`.** The binary ships in the relay image at
`/usr/local/bin/buzz-avnu-proxy` (Dockerfile already builds and copies it).
ECS overrides `command` to that binary. There is deliberately no separate
image variable — CD already passes `relay_image` on every apply, so one pin
covers both the relay and this proxy. Do not invent a second writer that could
un-pin CD (indexer needs its own ECR image because it is a different artefact;
this does not).

It is an HTTP service on port **8788**, following the indexer ingress pattern
(not paymaster egress-only):

- Own security group: ingress from the ALB on 8788 only
- Own Secrets Manager secret (`buzz-dev/avnu-proxy`) for `AVNU_API_KEY` and
  `PROXY_AUTH_TOKEN` — unmanaged version; **already exists in AWS** — import
  on first enable (no `aws_secretsmanager_secret_version`)
- ALB HTTPS listener rule (priority 110): host-header
  `paymaster.bitcoinmarkets.app` → avnu-proxy TG
- Health check `GET /health` → `{"status":"ok","service":"buzz-avnu-proxy"}`
- JSON-RPC: `POST /` and `POST /rpc` (Bearer `PROXY_AUTH_TOKEN` required —
  `BIND_ADDR=0.0.0.0:8788` is non-loopback)
- Default listener action stays the relay

The shared ACM certificate carries a SAN for `paymaster.bitcoinmarkets.app`
even while the service is off (so enabling does not wait on a cert
replacement). The Route53 A alias and listener rule appear only when
`avnu_proxy_enabled` is true.

Public URL for Wallet (after the host is live — not set in this stack PR):
**`AVNU_PROXY_URL=https://paymaster.bitcoinmarkets.app`**.

### Turning it on

Operator applies AWS after merge (this repo does not apply in CI for optional
services). Order matters — bootstrap first. Keep `avnu_proxy_enabled = false`
in committed tfvars until you are ready.

1. **Apply the bootstrap stack** (separate state; CI never applies it):

   ```bash
   terraform -chdir=infra/aws/bootstrap apply
   ```

   Grants `iam:PassRole` on `buzz-dev-avnu-proxy-execution` /
   `buzz-dev-avnu-proxy-task`, and denies the deploy role `GetSecretValue` on
   `buzz-dev/avnu-proxy`.
2. **Enable resources with `desired_count = 0`**:

   ```hcl
   avnu_proxy_enabled       = true
   avnu_proxy_desired_count = 0
   ```

3. **Import the existing secret** (look up the live ARN at apply time — do not
   hardcode the random suffix; name `buzz-dev/avnu-proxy`, account
   `618867225791`, region `eu-west-3`; ARN suffix was `-d4y5BS` at authoring
   time but verify):

   ```bash
   ARN=$(aws secretsmanager describe-secret \
     --profile alc --region eu-west-3 \
     --secret-id buzz-dev/avnu-proxy \
     --query ARN --output text)
   IMAGE=$(aws ecs describe-task-definition --task-definition buzz-dev-relay \
     --profile alc-tf --region eu-west-3 \
     --query 'taskDefinition.containerDefinitions[0].image' --output text)
   terraform import -var-file=dev.tfvars -var relay_image="$IMAGE" \
     'aws_secretsmanager_secret.avnu_proxy[0]' "$ARN"
   ```

4. Apply the main stack (creates SG, IAM, TG, listener rule, Route53, service
   at desired_count 0) with the usual `relay_image` pin.
5. Set `avnu_proxy_desired_count = 1` and apply again. The service does **not**
   use `ignore_changes = [desired_count]` (unlike paymaster), so this apply
   scales to one task.
6. Point Wallet at `AVNU_PROXY_URL=https://paymaster.bitcoinmarkets.app`
   (client/product change — not part of enabling Terraform).

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
manage that secret's version. The same Deny list covers the paymaster,
indexer, and avnu-proxy secrets. A second bootstrap role, `buzz-dev-indexer-ecr-push`, is
assumable only by `repo:adrienlacombe/the-situation-sdk:ref:refs/heads/main`
and may only push to ECR `buzz-dev-indexer` — no PowerUser, PassRole, or
Secrets Manager. CD is denied `iam:*` on both roles
(`NeverLetCdTouchItsOwnCredentials`).

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

**Markets indexer.** Product `INDEXER_URL=https://markets.bitcoinmarkets.app`
(required env or that public host). **No localhost default** — loopback was
listing-proof only. Listing/health need no auth. Never commit `ADMIN_API_KEY`
or `AVNU_API_KEY`. See `markets.tf.md`.

**AVNU proxy.** Product `AVNU_PROXY_URL=https://paymaster.bitcoinmarkets.app`
after the host is live (Wallet sets it — not desktop PRODUCT URLs in the
Terraform PR). **No loopback** — clients refuse it. `AVNU_API_KEY` stays in
`buzz-dev/avnu-proxy`. This is not `paymaster.tf`. See `avnu-proxy.tf`.

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
