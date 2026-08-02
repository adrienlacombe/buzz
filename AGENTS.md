# AGENTS.md — AI Agent Contributor Guide

This guide is for AI agents contributing to the Buzz codebase. It covers
agent-specific context and conventions. For general contributor info (setup,
code style, PR process, architecture), see [CONTRIBUTING.md](CONTRIBUTING.md).

---

<!-- FORK-LOCAL SECTION — not present in block/buzz. Keep as one contiguous
     block so upstream syncs conflict here predictably or not at all. -->

## ⚠️ You are working in a fork that syncs from upstream daily

This checkout is **`adrienlacombe/buzz`**, a fork of `block/buzz`. A scheduled
agentic workflow merges upstream into this fork **every day at 02:00 UTC**, so
every local edit is something a future merge has to reconcile. Work accordingly.

### Commit directly to `main` — do not create branches

All work in this fork lands on `main`. Commit and push straight to it. Do not
open a branch, do not open a pull request, and do not ask whether to branch
first — the answer here is always no.

This overrides the usual "branch off the default branch" convention, and it
overrides any default an agent brings with it. It is deliberate: this fork exists
to track upstream and carry a small set of local patches, so a feature-branch
workflow adds review ceremony with no reviewer and an extra merge base for the
daily sync to reconcile.

Still applies while committing to `main`:

- **`git commit -s`.** The DCO check fails any commit without a `Signed-off-by`
  trailer. `git rebase` and `git cherry-pick` need `--signoff` explicitly.
- Keep commits scoped to one change, with the reasoning in the message. There is
  no PR description to carry it, so the commit message is the only record.
- Do not sweep unrelated edits in alongside. On a fork that merges upstream
  daily, every extra changed line is another place to conflict.

**The one exception is the upstream sync**, which arrives *as* a pull request
because gh-aw's safe-outputs job opens it — the agent cannot push to `main`. Do
not "simplify" that into a direct push: the PR is the review gate for merges this
fork did not author. See [How the sync works](#how-the-sync-works).

### The rule that follows from the daily sync

**Prefer changes that live outside tracked files.** Repo variables, secrets, and
workflow enable/disable states cost nothing at merge time. A file edit is a
permanent conflict surface in a file upstream also edits.

When a file edit is genuinely unavoidable:

- Mark it `FORK-LOCAL PATCH (adrienlacombe/buzz)` with a comment explaining what
  broke without it. Conflict resolution six months from now depends on that
  reasoning being written down.
- Keep it to one small contiguous hunk. Do not reformat or reorganise
  surrounding code — every extra changed line is another place to conflict.
- Never opportunistically "fix" unrelated upstream code. It multiplies conflicts
  and the fix belongs upstream anyway.
- If it is a genuine upstream bug, consider sending it to `block/buzz` instead of
  carrying the patch. A merged upstream fix removes divergence permanently.

**When upstream lands a fix that makes a fork patch redundant, delete the fork
patch** — and delete its row from the table below in the same commit, so the table
never describes a patch that is no longer there. Carrying both is permanent
conflict surface for nothing.

That happened to `desktop/src-tauri/src/linux_media.rs` in the 2026-07-31 sync.
The fork carried a module-level `cfg_attr(not(linux), allow(dead_code))` because
`PROD_ORIGIN`, `DEV_ORIGIN` and `is_trusted_media_origin` are reachable only from
the `cfg(linux)` `enable_media_capture` and the tests, so `clippy -- -D warnings`
failed on the lib target on macOS and broke the pre-push hook for Mac developers.
Upstream's `36571f4ad` (#3811) added per-item allows covering exactly those three
items, so the fork patch was dropped. **Verify before deleting** — the check that
settled it was running `cargo clippy --manifest-path desktop/src-tauri/Cargo.toml
--all-targets -- -D warnings` on macOS with the patch removed, not reading the
upstream diff and assuming.

### How the sync works

The sync runs in **two stages**, and which one produced a PR determines what you
have to check when reviewing it.

**01:30 UTC — `.github/workflows/upstream-sync-merge.yml`, plain git, no AI.**
Fetches `block/buzz`, exits if not behind, exits if a sync PR is already open,
then runs `git merge --no-ff upstream/main`. Clean merges are pushed by the runner
and opened as an `[upstream-sync]` PR. Conflicts abort and hand off to 02:00. This
is the common path and it costs no AI credits.

**05:00 UTC — `.github/workflows/upstream-sync.md`, Copilot via gh-aw.** Stops
immediately if a pushed sync branch already contains `upstream/main`, so it cannot
stack a second PR on the 01:30 one. Otherwise it merges, resolves conflicts, and
writes the *Conflicts* and *Needs a human look* sections. It runs with
`contents: read` and never pushes.

**Why 05:00 and not 02:00:** GitHub delays scheduled runs under load. On
2026-07-31 the 01:30 job did not start until 02:17 — after the 02:00 slot this
stage used to hold — so the deterministic stage no longer ran first and the
ordering both stages depend on was inverted. A 30-minute gap is not a guarantee;
3.5 hours is the margin.

**The handoff only works if this workflow is enabled.** It was
`disabled_manually` on 2026-07-31, so when the 01:30 stage hit conflicts and
handed off, nothing picked them up: no PR, no issue, and the fork sat 17 commits
behind in silence. Both stages reported success, because a conflicted handoff is
success for the first stage. `gh workflow list --all` is the only place that
state is visible — check it if syncs go quiet.

**Only the 01:30 stage preserves upstream history**, and that distinction is the
whole reason it exists. gh-aw moves the agent's work into its PR job as a git
*patch* (`/tmp/gh-aw/aw-*.patch`), and a patch carries file deltas but not a merge
commit's second parent. So an agentic sync lands upstream's content under a
single-parent commit: the merge base never advances, GitHub keeps reporting the
fork N commits behind, and every later run re-merges from that same stale base and
re-resolves the same conflicts. This is not a model failure — Copilot merged
correctly and described it honestly in PR #7 — it is structural, which is why the
fix was to move the push rather than change engines. PR #7 hit exactly this and was
repaired by hand in `3ce7c8adc`.

gh-aw's own logs state this outright rather than leaving it to be inferred — the
2026-07-31 run printed `pushSignedCommits: merge commit … detected, refusing
unsigned push fallback`, then `Rewriting bundled commits to a single linear commit
for signed push compatibility`. The flattening is deliberate on gh-aw's side, so
it will not be fixed by configuration.

**If an agentic (05:00) PR ever lands, re-merge upstream by hand afterwards** or
the counter stays stuck: `git merge upstream/main` on `main`, resolve, push.

**The patch transfer also drops the executable bit, and it surfaces as a red CI
run rather than as an obvious defect.** The 2026-08-01 agentic PR (#12) carried
upstream's new `scripts/test-desktop-release-authorization.sh` and
`scripts/verify-desktop-release-authorization.sh` at mode `100644` where upstream
has `100755`, so `scripts/test-release-ref-contract.sh` could not execute them:
`Detect Changed Paths` died with `Permission denied` and exit 126, which then
skipped every downstream job. Upstream's content was fine — the same contract
script passes on a hand merge of the same range. **A mode-only change is a
zero-line entry in `git diff --stat`**, so a diff skim will not show it; when a
sync adds a script, compare `git ls-tree <ref> -- <path>` against `upstream/main`.

**When the agentic stage files an issue, read the bottom of it before assuming it
gave up.** gh-aw converts an intended pull request into an issue when the push
fails, keeping the whole PR body — resolutions included — and appending the git
error as a note. Issue #8 looked like a conflict escalation and was actually a
completed, correct resolution that could not be pushed for want of a
`workflow`-scoped token. The work is recoverable from the run's bundle artifact.

Issue **#1 `[aw] No-Op Runs`** collects a comment per quiet day from the agentic
stage. Silence there means that workflow has stopped working — it says nothing
about the 01:30 stage.

**Reviewing a sync PR:**

- Merge with a **merge commit**, never squash — a squash drops the second parent
  and undoes the 01:30 stage's entire purpose.
- Read *Conflicts* and *Needs a human look* if it came from the agentic stage.
- **A clean merge is not a correct merge, and neither stage can tell you
  otherwise.** Upstream #3568 auto-merged with no conflict and still made
  `assemble-manifest`'s gate unreachable in a fork while stranding
  `release-macos-unsigned` on the old release model — two silent breakages in one
  file, caught only by reading the diff (`3ce7c8adc`). Always read the diff at the
  patched files below, `release.yml` above all.
- When a conflict lands in one of those files, the resolution is nearly always
  *keep our marked line, take upstream's everything else*.

### Fork-local file patches

These files differ from `block/buzz`. Each carries a `FORK-LOCAL` comment in
place.

| File | Change | Why |
|------|--------|-----|
| `.github/workflows/upstream-sync.md` + `.lock.yml` | new | The agentic (02:00) sync stage. Edit the `.md` and run `gh aw compile upstream-sync`; the body is **not** inlined into the `.lock.yml`, which stores only a `body_hash`, so a body-only edit shows up as a one-line lock diff |
| `.github/workflows/upstream-sync-merge.yml` | new | The deterministic (01:30) sync stage — the one that preserves the merge parent. Plain git, no AI. Optional `SYNC_PUSH_TOKEN` secret: a branch pushed with `GITHUB_TOKEN` does not start new workflow runs, so set a PAT if CI stops firing on sync PRs |
| `.github/workflows/upstream-sync-ci-status.yml` | new | Labels an open sync PR `sync-ci-green`/`sync-ci-red` once checks settle, and re-requests the Copilot review that gh-aw's `reviewers:` fails to attach. Deliberately does not merge |
| `migrations/0027_wallet_binding_fts.sql`, `0028_wallet_binding_fts_kind_move.sql` | new, and **kept after the feature was removed** | Search exclusions for the withdrawn NIP-SW wallet binding. They have already run on live databases and sqlx checksums applied migrations, so deleting them breaks startup validation. What they leave behind — a `search_tsv` expression excluding a kind nobody publishes — is inert, and unwinding it would rewrite a generated column across the whole events table for nothing. **Never edit or delete an applied migration**; add a follow-on |
| `crates/buzz-db/src/migration.rs` | `migrations.len()` assertion is 28, not upstream's 26 | Counts embedded migrations, so it moves whenever the fork adds one. `0027` landed without bumping it and left the test failing on `main`; fixed in PR #9. A one-integer conflict on every upstream migration — take upstream's count and add the fork's two |
| `.github/workflows/macos-canary.yml` | new; `push` trigger on `main` with desktop path filters | Unsigned macOS canary; upstream only has a *signed* one, which a fork cannot run. Builds automatically when `desktop/**`, `crates/**` or the root `Cargo.*` change, so the newest artifact always matches `main` — it was dispatch-only, and the sole artifact went 13 commits stale. Free: the repo is public, so GitHub-hosted macOS runners are unbilled. Stages the artifact and the usage notes under the product name read from `tauri.conf.json`, not a hardcoded one, so the brand rename below cannot publish a build under the old name. Sets `signingIdentity: "-"` in its inline config and runs **without** `--no-sign`, which would silently discard it; asserts the bundle signature of the `.app` inside the mounted DMG |
| `.github/aw/actions-lock.json` | new | gh-aw action SHA pins |
| `.gitattributes` | `*.lock.yml linguist-generated` | Added by `gh aw init` |
| `ci.yml` | mesh-llm rev read from `desktop/src-tauri/Cargo.lock` | The two locks pin mesh-llm independently (desktop is outside the root workspace) and can name different revs — at the time of the patch, root `tag=v0.73.1` (`43103c5c`) vs desktop `rev=f455d493`. The step fetches the *desktop* manifest, so the root rev names a checkout never fetched. Upstream is masked by a warm cache — the step is skipped on cache hit. **Since the 2026-07-31 sync both locks pin `tag=v0.74.0` (`e60b2fe4`), so the patch is a temporary no-op — do not delete it.** The locks stay independent; the next bump that moves one and not the other re-breaks the root-lock version |
| `docker.yml` | `PUSH_GATEWAY_IMAGE` override; owner-correct attestation hint | Push-gateway image was hardcoded to `ghcr.io/block/buzz-push-gateway` in nine places, so `GHCR_IMAGE` could not retarget it |
| `release.yml` | `RELEASE_REPO` guard on `setup` + `release-linux`; `BASE` and `BUZZ_UPDATER_ENDPOINT` derive from `github.repository`; `assemble-manifest` asserts on job results instead of counting platforms; `release-macos-unsigned` runs without `--no-sign`, sets `BUZZ_MACOS_ADHOC_SIGN=1`, and asserts the bundle signature | Guards were pinned to `block/buzz`; the updater URLs were hardcoded to Block's releases, so a fork verified its artifacts against Block's rolling release and shipped builds polling Block for updates. The `-ge 3` platform count was unreachable with both macOS jobs skipped, so `latest.json` was never published. `--no-sign` suppressed updater signing too, so the `.app.tar.gz` shipped with no `.sig`; without ad-hoc signing the bundle had no signature at all and macOS called it damaged — see [Desktop auto-update](#desktop-auto-update-linux--windows--works). **Do not name the release-upload command anywhere in this file, even in a comment:** `scripts/test-release-ref-contract.sh` counts occurrences of that string and requires exactly two |
| `release.yml` + `macos-canary.yml` | `BUZZ_DESKTOP_BUILD_AUTO_CONNECT_DEFAULT_RELAY: "1"` on the build step of the three fork-runnable lanes and the canary | Skips the "Join or create a community" picker and auto-creates the single allowlisted community. This is **upstream's own opt-in**, for builds whose default relay is reviewed and fixed — no source change was needed. It works because release builds already default to `wss://relay.bitcoinmarkets.app` (`relay.rs` → `relay/allowlist.rs`) and `shouldAutoConnectDefaultRelay` accepts any non-loopback `ws(s)` URL. `option_env!`, so it is **compile-time**: absent at build time it silently does nothing. Deliberately not set on the two `block/buzz` macOS lanes, and irrelevant in debug builds where the loopback default correctly keeps the picker. Assert it with the `#[ignore]`d `compiled_flag_matches_expected` test and `BUZZ_TEST_EXPECTED_AUTO_CONNECT_DEFAULT_RELAY` |
| `desktop/scripts/build-release-config.mjs` | `BUZZ_MACOS_ADHOC_SIGN=1` emits `bundle.macOS.signingIdentity: "-"` | Ad-hoc bundle signing for the one macOS lane nothing else signs. Opt-in and off by default, so the two `block/buzz` lanes still reach `block/apple-codesign-action` unsigned — setting it unconditionally would sign a bundle that is about to be re-signed. It has to be config rather than a post-build `codesign`, because Tauri builds the DMG in the same invocation |
| `linux-canary.yml`, `windows-canary.yml` | `RELEASE_REPO` guard | Were pinned to `block/buzz` |
| `infra/aws/` | new directory | Terraform deploying the relay to AWS account `618867225791` (`eu-west-3`) on ECS Fargate + RDS + ElastiCache + S3 + EFS, serving `wss://relay.bitcoinmarkets.app`. Upstream deploys via `deploy/charts/buzz` (Helm) and has no Terraform, so this adds only new paths and should never conflict. See [`infra/aws/README.md`](infra/aws/README.md) |
| `.github/workflows/deploy-aws.yml` | new | Continuous deployment of the relay to AWS on every push to `main`. Runs after `docker.yml` via `workflow_run`, authenticates by OIDC (no stored keys), and applies Terraform with the commit's immutable `:sha-<7>` image |
| `desktop/src-tauri/src/relay/allowlist.rs` | new | Single-relay host allowlist. Upstream is multi-community by design; this fork ships a client that reaches only `relay.bitcoinmarkets.app`. **Lives under `relay/`, not at the crate root** — see the `relay.rs` row |
| `desktop/src-tauri/src/native_websocket.rs` | allowlist call in `open_connection` | The transport is the one path every relay session takes, so a host restriction there cannot be bypassed from the UI |
| `desktop/src-tauri/src/relay.rs` | release builds default to the allowlisted relay; also declares `pub mod allowlist;` | Without the default a release build uses `ws://localhost:3000`, which the allowlist then rejects — a client that cannot connect at all. The module is declared *here* because upstream's `lib.rs` sits at exactly the 1000-line desktop file-size ratchet limit with no headroom, so the fork's two-line `mod` block there failed `just desktop-check` as soon as upstream added anything (it did, in the 2026-08-01 sync). `lib.rs` now carries no fork patch at all |
| `mobile/lib/shared/relay/relay_allowlist.dart` | new | Mobile counterpart. Skips enforcement under `flutter test` (`FLUTTER_TEST`) because upstream tests use `wss://relay.example.com`; editing those 13 files would be a large permanent conflict surface |
| `mobile/lib/shared/relay/relay_socket.dart` | allowlist call in `connect()` | Transport choke point, as on desktop |
| `mobile/lib/shared/relay/relay_validation.dart` | allowlist call after the shape checks | One hunk covers all four invite/deep-link call sites; placed after the existing checks so malformed input keeps its original error |
| `mobile/lib/features/pairing/pairing_provider.dart` | allowlist call in `_validateRelayUrl` | QR pairing is a separate entry point from invites |
| `mobile/lib/shared/relay/relay.dart` | export of `relay_allowlist.dart` | Barrel re-export |
| `desktop/src-tauri/tauri.conf.json` | `productName` `Buzz` → `BitcoinMarkets`; `identifier` → `app.bitcoinmarkets.desktop`; deep-link scheme → `bitcoinmarkets` | Names the `.app` bundle and the DMG filename. **The one divergence carrying no in-file marker** — JSON takes no comments — so a sync reviewer has only this row to go on. The identifier and scheme changes are what stop this fork sharing state with an upstream Buzz install; see [Splitting state from upstream Buzz](#splitting-state-from-upstream-buzz) |
| `desktop/src-tauri/src/app_state_keyring.rs` | `RELEASE_KEYRING_SERVICE` = `bitcoinmarkets-desktop`, and it joins the canonical list in `migration_marker_name` | The keychain service name is a constant that does **not** key off the bundle identifier (`secret_store.rs:50` says so explicitly), so renaming the identifier alone would split the app-data directory while leaving both apps on one keychain entry — identity and store then disagree, which is worse than sharing both |
| `desktop/src-tauri/src/deep_link.rs` | `DEEP_LINK_SCHEME` const; handler accepts `bitcoinmarkets` **and** `buzz` | Only `bitcoinmarkets` is OS-registered, which is the collision fix; `buzz://` stays accepted inbound because such links already exist in message history. Emission is exclusive, acceptance is not |
| `desktop/src-tauri/src/lib.rs` | `mod relay_allowlist;` (see below); single-instance argv accepts both schemes | Matches the `deep_link.rs` guard — a duplicate launch forwards either scheme |
| `desktop/src/features/messages/lib/messageLink.ts` + `remarkMessageLinks.ts` + `shared/api/inviteHelpers.ts` | emit `bitcoinmarkets:`, accept both | `messageLink.ts` builds "Copy link" URLs, so it must emit the registered scheme or copied links open the wrong app. The other two are acceptors: `remarkMessageLinks` detects bare URLs in message text (a miss renders a valid link as inert text) and `inviteHelpers` parses invite links. Upstream's `(?:buzz\|buzz)` alternation was degenerate — leftover from its own rename |
| `web/src/shared/lib/deep-link.ts` | new | Single source of truth for the scheme in the web client, which is what hands these URLs to the OS. Additive so it cannot conflict; the three call sites (`InvitePage.tsx` ×2, `ConnectButton.tsx`) import it rather than inlining fork-local strings into upstream components. `InvitePage` also says "Accept invite in BitcoinMarkets" |
| `desktop/src-tauri/Info.plist` | `CFBundleDisplayName`, `CFBundleName` and the three `NS*UsageDescription` strings → `BitcoinMarkets` | `productName` only renames the `.app` directory, the DMG and the mounted volume. These keys are what macOS displays: Finder reads `CFBundleDisplayName`, the menu bar reads `CFBundleName`, and the usage descriptions are quoted verbatim in system permission prompts. Verified against a built canary before patching — the bundle was `BitcoinMarkets.app` while `CFBundleName` was still `Buzz`, so the app asked for the microphone as "Buzz". `CFBundleIdentifier` and the `buzz-desktop` executable name stay |
| `mobile/ios/Runner/Info.plist` | `CFBundleName` and the three `NS*UsageDescription` strings → `BitcoinMarkets` | The xcconfigs below set `CFBundleDisplayName` (home-screen label); `CFBundleName` is the shorter name iOS falls back to in Settings, and it was still `Buzz`. Usage descriptions appear verbatim in iOS permission prompts |
| `mobile/ios/Flutter/Debug.xcconfig`, `Release.xcconfig` | `APP_DISPLAY_NAME = BitcoinMarkets` | iOS home-screen name, debug and release |
| `mobile/android/app/build.gradle.kts` | `app_name` resValue → `BitcoinMarkets`, in `defaultConfig` and the worktree-debug branch | Android launcher label. Two hunks because the worktree label composes onto the same string |
| `scripts/mobile-worktree-overrides.sh` | branch-labelled debug name | Generates the gitignored per-worktree `APP_DISPLAY_NAME` |
| `scripts/test-mobile-worktree-overrides.sh` | assertions derive the production name from `mobile-worktree-overrides.sh` instead of matching the literal `Buzz` | Four assertions hardcoded `Buzz` and **failed CI on `main` for three commits** after the rename (`ff5e83c28`…`684a15f50`), cascading into `Desktop` and `Desktop E2E Integration` through their gate steps. Deriving the name tests the contract the file is for — release unlabelled, debug labelled, iOS and Android agreeing — so a future rename cannot fail it for the wrong reason |

The `RELEASE_REPO` pattern means opting in or out is a **variable** change, not a
file edit — delete the variable to restore upstream behavior without reverting
any commit.

The brand rename is **display names only**. Bundle identifiers are deliberately
untouched (`xyz.block.buzz.app`, `com.buzz.buzzMobile`) so existing installs keep
upgrading and the desktop app-data directory — which holds the `identity.key`
keyring fallback — does not move. `mobile/pubspec.yaml`'s `name: buzz` is the Dart
package name behind every `package:buzz/...` import, not a user-visible string, and
must stay. When a sync conflicts in one of these files, keep our marked line and
take upstream's everything else; when it conflicts in `tauri.conf.json`, keep
`productName` and take upstream's `version`.

`signed-macos-canary.yml` and `release.yml`'s `release` job still hardcode
`Buzz.app`, left unpatched on purpose: both are guarded
`github.repository == 'block/buzz'` and cannot run here, so patching them would add
conflict surface for code that never executes. They would break if ever enabled.

`infra/aws/` is additive-only by construction — it edits no upstream file. If an
upstream sync ever conflicts there, something has gone wrong; do not resolve it by
reformatting the Terraform.

### Splitting state from upstream Buzz

This fork and upstream Buzz can be installed side by side, and until 2026-08-01 they
shared everything that matters: the same app-data directory, the same keychain
entry, and the same URL scheme. One account, one settings store, and `buzz://`
links opening whichever app macOS felt like.

Three things had to move, and only the first is config:

| Collision | Keyed by | Now |
|-----------|----------|-----|
| App-data dir (`identity.key`, settings, window state, localStorage) | `identifier` in `tauri.conf.json` | `app.bitcoinmarkets.desktop` |
| OS keychain entry | a **constant** in `app_state_keyring.rs`, not the identifier | `bitcoinmarkets-desktop` |
| `buzz://` deep links | `plugins.deep-link.desktop.schemes` | `bitcoinmarkets` only |

The keychain one is the trap. `secret_store.rs:50` states that the service name
"does not key off the bundle identifier", so changing the identifier alone splits
the data directory while leaving both apps writing one keychain entry — the
identity and the store that guards it then disagree, which is worse than sharing
both. **They move together or not at all.**

**There is deliberately no migration.** `migrate_legacy_app_data_dir` only maps
`xyz.block.buzz.app*` → `xyz.block.sprout.app*`, so the new identifier matches
neither prefix, `legacy_app_data_dir` returns `None`, and nothing is copied. That is
intentional: this fork wanted a clean separation, not an inherited identity. The
practical consequence is that **an existing install looks like a fresh account
after this change** — the old data still sits under `xyz.block.buzz.app`, unread.
Anyone who wants it carried over has to write that migration, and the hook to
extend is `legacy_app_data_dir`.

Bundle identifiers were originally left alone on purpose, so installs kept
upgrading and the app-data directory holding the `identity.key` keyring fallback
did not move. That reasoning was about *upgrades*; coexisting with upstream was not
a case it considered.

### Fork-local event kinds

**Two: `KIND_SPONSOR_REQUEST = 30900` and `KIND_SPONSOR_RESULT = 30901`**, the
sponsorship protocol between a client and `buzz-paymaster`. `kind.rs` was briefly
byte-identical to upstream after NIP-SW was removed; these bring the divergence
back, which is the price of routing sponsorship over Nostr instead of an HTTP
endpoint on a funded service.

**30900 is reused deliberately.** The withdrawn wallet binding held it for a day, so
migration `0028` already excludes 30900 from full-text search — an exclusion the
request wants for the same reason, since its payload carries account addresses and
calldata. Reusing the integer inherits that without another `search_tsv` rewrite
across the whole events table. A stale binding still stored at 30900 fails sponsor
payload validation and is ignored, so the reuse fails closed. `30901` carries only a
status and a transaction hash, so it needs no exclusion.

**Kinds `30900`–`30999` are reserved for this fork.**
Upstream's parameterized-replaceable kinds cluster at `30174`–`30178` and grow
upward, so a fork constant placed near them gets claimed sooner or later. Put a new
fork kind in the reserved block, not next to the upstream kind it relates to.

The history is worth keeping because it cost real work twice over. The fork put
NIP-SW's Starknet wallet binding at `30178`; upstream then shipped
`KIND_TEAM_CATALOG = 30178` (#3358), and the collision was two unrelated schemas on
one integer in the same crate — `ingest_event_inner` ran both the attestation
verifier and `validate_team_catalog_envelope` against every such event, so one
always rejected the other's traffic. Text merging cannot fix that. The rule that
came out of it: **upstream keeps the contested integer, the fork moves.** The
binding moved to `30900` in PR #9, and the whole feature was withdrawn afterwards in
favour of the Nostr key controlling a Starknet account directly.

Three things generalise from it, for whenever a fork kind reappears:

1. **Moving a kind is a wire-format change.** Stored events are not rewritten and
   pinned clients stop matching. Check for existing events first.
2. **It may be a search-exclusion change too, which is the easy miss.** An FTS
   exclusion is a `kind = …` literal baked into a `search_tsv` generated column.
   Applied migrations never re-run and `sqlx::migrate!` validates their checksums,
   so the original file cannot be edited — that fails relay startup on a version
   mismatch. The renumber needed a *follow-on* migration (`0028`) to peel `0027`'s
   wrapper and re-wrap on the new integer. Any future move carrying an exclusion
   needs the same.
3. **Adding a migration means bumping `migrations.len()`** in
   `buzz-db/src/migration.rs`. `0027` did not, and left that assertion failing on
   `main`.

The full checklist for adding a fork-local kind: the constant in
`buzz-core/src/kind.rs`, its `is_parameterized_replaceable` assertion,
`SHARED_GATED_KINDS` if shareable, the relay's `required_scope_for_kind` and ingest
branch, the SDK builder in `buzz-sdk/src/builders.rs`, any `buzz-cli` subcommand,
and `desktop/src/shared/constants/kinds.ts` plus
`mobile/lib/shared/relay/nostr_models.dart`, which must stay in sync. The assertion
and the migration count are the two that were missed last time.

### Starknet accounts

This fork ships **one** Starknet model: `contracts/src/account.cairo`, an account
whose owner is a Nostr x-only pubkey, validating BIP-340 Schnorr signatures on
chain. The Nostr key *is* the account signer.

The alternative — NIP-SW, where an external wallet held the funds and a kind:30900
event carried an on-chain attestation binding it to a Nostr pubkey — was
implemented, deployed as far as relay-side ingest verification, and then removed
entirely. Do not reintroduce it piecemeal; `docs/nips/NIP-SW.md`, `wallet_binding`,
`snip12`, `starknet_verify` and the `wallet message`/`publish`/`get`/`lookup`
subcommands are all gone.

**The cost of the model that remains is per-transaction and does not amortise**:
~0.78 STRK of BIP-340 verification before a transaction does anything useful,
measured on mainnet. See [`contracts/DEPLOYMENTS.md`](contracts/DEPLOYMENTS.md) for
the numbers, the declared class hash, and why address derivation is confirmed by
the sequencer without a deployment having happened.

### Sponsorship (`buzz-paymaster`)

Because a fresh account cannot pay its own ~0.78 STRK of BIP-340 verification, it
cannot act at all until someone else pays first. `buzz-paymaster` is that someone:
it subscribes to the relay as an ordinary client, watches kind:30900, and submits a
**single atomic multicall** that deploys the account (via the UDC, from a funded
sponsor account) and runs the user's SNIP-9 payload in one transaction.

Four properties are load-bearing. Each was chosen against a specific failure, so
changing one needs the failure re-considered rather than the code re-read:

- **No inbound network surface.** A funded service that connects *out* and
  subscribes is a much smaller target than one exposing an authenticated API, and it
  inherits NIP-42 auth plus community membership as the gate on who may ask. This is
  the reason sponsorship is a Nostr kind and not an HTTP endpoint.
- **The account address is derived from the *event author*, never from the
  payload.** The relay has already verified the event signature, so the author is
  attested. Accepting a caller-supplied address would let one member aim a sponsored
  deployment at another member's account, or at an arbitrary contract.
- **`deploy_from_zero` must be `1`** in the UDC calldata (`udc_deploy_calldata`).
  Address derivation uses `deployer_address = 0`; with the flag false the UDC's own
  address is mixed into the hash and the account lands where nobody derived it,
  holding whatever the user was told to send. It is the single most consequential
  value in the crate and has its own test.
- **Deploy must come first in the multicall.** Reversed, the execute call targets an
  address with no contract at it and the whole transaction reverts *after* the
  sponsor has committed to the fee.

#### Paying twice is the failure mode to design against

A stored request is replayed to every new subscription, so "service what arrives" is
wrong on its own. Three mechanisms stack, cheapest first:

1. **The validity window**, checked before any chain query. An expired request is
   refused for free — this is what makes replaying stored requests safe at all, and
   it is why the sponsor subscribes with **no `since`**: a request published while it
   was down must still be serviced.
2. **A dedupe set**, rebuilt at every connect from the sponsor's own published
   kind:30901 results. This is what protects a restarted process, and it is why
   `run_once` drains the results subscription to EOSE *before* subscribing to
   requests — and fails rather than proceeding if EOSE never comes.
3. **The single-use on-chain nonce.** The backstop, and it costs a reverted
   transaction's fee. Not the plan.

The result's `d` tag is **`<requester_pubkey>:<nonce>`**, not the nonce alone.
Results are authored by the sponsor, so their replaceable identity is
`(sponsor_pubkey, 30901, d)`; a nonce-only key would put two members who both chose
`0x1` in one slot, silently overwriting one member's result *and* dropping an entry
from the sponsor's record of what it had already paid for.

**The hazard that remains:** a crash between submitting and publishing loses the only
record of the payment, since the record *is* the published event. Mechanism 1 bounds
it — a client asking for a short window is protected by construction, one asking for
hours is not. Closing it properly needs durable state written before submission.

Run **one** instance. Two would both rebuild the same dedupe set and both service a
request that arrives before either has published; there is no lock.

#### Fee estimation is the anti-drain guard, not a price check

There is **no per-member quota** — a deliberate choice. What stands in its place is
that `LocalSubmitter::submit` calls `starknet_estimateFee` before sending, and
estimation runs the whole multicall including `__validate__` and
`execute_from_outside_v2`. So a request with a bad BIP-340 signature, a spent nonce,
a closed window or a reverting call **errors during estimation and is refused for
free** instead of costing a reverted transaction's fee. Without it, any member could
drain the sponsor with requests that were never going to succeed.

Two things sit on top:

- **The chain-id check** (`ChainConfig::chain_id_matches`), which is cheaper still —
  no RPC at all. A request signed for another network embeds that chain id in its
  SNIP-12 hash, so it could only revert here. The chain id comes from the *node* via
  `LocalSubmitter::from_env`, which returns it for `ChainConfig`, so the check and
  the signing chain id are the same number by construction rather than by two config
  values agreeing.
- **A per-transaction fee ceiling** (`BUZZ_PAYMASTER_MAX_FEE_FRI`, default 10 STRK).
  The calls in a request are the *user's* and arbitrary, so estimation can correctly
  report an enormous number. The ceiling is checked against the **padded bound**
  (1.5x gas × 1.5x price = 2.25x the estimate), because the bound is what the
  sequencer is authorised to take — not the estimate. `TIP` is 0 for the same reason:
  a tip is fee outside the gas bounds, and any non-zero value makes the ceiling an
  underestimate.

**Submissions must not overlap.** The account nonce is read from the pre-confirmed
block and consumed by the transaction that follows, so two in flight would collide
and the second would be rejected. The service loop is sequential by construction;
this is the second reason to run one instance.

Spending authority enters through exactly one boundary, the `Submitter` trait, so
the funded key is invisible to every other part of the crate. `Config::from_env`
reads only the sponsor's *Nostr* key (relay identity, spends nothing) and its
`Debug` impl is hand-written to keep the secret half out of logs; the Starknet key is
read in `submitter.rs` alone, handed straight to the signer, and no error path
formats it.

The daemon is `cargo run -p buzz-paymaster`. It listens on **no port** — it connects
out — and its required environment is documented at the top of
`crates/buzz-paymaster/src/main.rs`.

### Repo settings (no file changes — preferred mechanism)

| Setting | Value | Why |
|---------|-------|-----|
| `GHCR_IMAGE` | `ghcr.io/adrienlacombe/buzz` | Retarget relay images to this namespace |
| `GHCR_PUSH_GATEWAY_IMAGE` | `ghcr.io/adrienlacombe/buzz-push-gateway` | Same for the push gateway |
| `RELEASE_REPO` | `adrienlacombe/buzz` | Opts this fork into the guarded release/canary jobs |
| Issues | **enabled** | Off by default on forks; the sync workflow's conflict-escalation output needs them |
| `Auto-tag on Release PR Merge` | **disabled** | Fires on every merged same-repo PR. A sync PR carrying a `deploy/charts/buzz/Chart.yaml` version bump hits its default lane and tries to mint a token from the `BUZZ_RELEASE_TAGGER` GitHub App, which does not exist here |
| `sprig-latest` release | **created manually** | `sprig.yml`'s rolling lane calls `gh release edit sprig-latest` with no create-if-missing fallback. Forks inherit no releases, so it failed on every push to `main` until the release existed |
| Code scanning | **default setup**, weekly, default query suite | CodeQL over `actions`, `javascript-typescript`, `python`, `rust`. Upstream ships no `codeql.yml`, so this is a setting rather than a workflow file — nothing to conflict. Triage below |
| `Sync this fork with upstream` workflow | **must stay enabled** | Found `disabled_manually` on 2026-07-31. It is the conflict-resolution half of the sync, so while disabled every conflicted merge was silently dropped — the 01:30 stage aborts and hands off, and nothing was listening. Neither run goes red, so this is invisible except via `gh workflow list --all`. Re-enable with `gh workflow enable "Sync this fork with upstream"` |
| `SYNC_PUSH_TOKEN` secret | **PAT with `workflow` scope — required** | Without it, no sync touching `.github/workflows/**` can be pushed by either stage: `refusing to allow a GitHub App to create or update workflow .github/workflows/ci.yml without 'workflows' permission`. There is no `workflows` key in Actions `permissions:`, so this cannot be granted to `GITHUB_TOKEN` — a PAT is the only route. Upstream edits its workflows regularly, so this blocks routine syncs, not rare ones. It is also what makes CI fire on the sync PR at all |

Secrets set here: `COPILOT_GITHUB_TOKEN` (sync engine),
`TAURI_SIGNING_PRIVATE_KEY` + `_PASSWORD` and `BUZZ_UPDATER_PUBLIC_KEY` (fork's
own throwaway updater keypair — unrelated to Block's).

### Code scanning triage

All 12 alerts from the first CodeQL run were in files **byte-identical to
upstream** — no fork-local code was implicated — and all are dismissed. Recorded
here because dismissals key to an alert number: a sync that re-touches these
files can raise the same finding under a new number, and without this table the
analysis gets redone from scratch.

| Rule | Where | Verdict |
|------|-------|---------|
| `rust/hard-coded-cryptographic-value` | `buzz-core/src/pairing/crypto.rs:55` | Empty HKDF salt, permitted by RFC 5869 §3.1 and fixed by NIP-AB. The IKM is a 32-byte CSPRNG session secret, so the salt adds nothing; domain separation comes from the `info` string. **False positive** |
| `rust/hard-coded-cryptographic-value` | `buzz-core/src/pairing/session.rs:114` | `[0u8; 32]` overwritten by `rand::fill` on the next line before any use — an array initializer, not a salt. **False positive** |
| `rust/hard-coded-cryptographic-value` ×3 | `desktop/src-tauri/src/commands/identity.rs:528,566,582` | The `nonce` argument in unit tests inside `#[cfg(test)] mod nostr_identity_binding_tests`. **Used in tests** |
| `py/clear-text-storage-sensitive-data` ×3 | `benchmarks/harbor-buzz-orchestra/scripts/benchmark.py:186,236,275` | Secrets the script generates itself (`secrets.token_urlsafe`/`token_hex`) guarding a throwaway local docker stack, written to files created mode `0600`. docker-compose needs a plaintext `.env`. **Won't fix** |
| `js/xss-through-dom` ×2 | `desktop/src/features/agents/ui/AgentCreationPreview.tsx:752,889` | Sink is `<img src>`, a passive context — `javascript:` URLs do not execute there and SVG loaded via `<img>` cannot run script. The value is the avatar URL the user typed into their own client. **False positive** |
| `js/incomplete-multi-character-sanitization` | `desktop/src/features/projects/ui/ProjectReadmePanel.tsx:35` | `htmlInlineToMarkdown` is a markdown normalizer, not a sanitizer. **False positive** |
| `js/double-escaping` | `desktop/src/features/projects/ui/ProjectReadmePanel.tsx:26` | **A real bug**, not exploitable. See below |

The one genuine defect is `decodeHtmlEntities`, which decodes `&amp;` *before*
`&lt;`, so `&amp;lt;` becomes a literal `<`. A README containing escaped HTML
renders wrong. It is not an XSS: the output goes to `<Markdown>` →
`react-markdown` 10 with **no `rehype-raw`**, which never renders raw HTML, and
the custom `messageLinkUrlTransform` still delegates to `defaultUrlTransform` for
every scheme except `buzz://message?…`, so dangerous protocols stay blocked.

Not patched here on purpose: a one-line reorder in an upstream file, for a
cosmetic bug, is permanent conflict surface — the fix belongs in `block/buzz`.
**If a sync ever makes `desktop/src/shared/ui/markdown/nodeCache.ts` render raw
HTML, or replaces `urlTransform` without delegating, this stops being cosmetic
and both README alerts become live.** That is the thing to re-check, not the
alert numbers.

### What cannot work in a fork

Do not spend time trying to make these pass; they are structural, not
misconfiguration.

- **macOS signing / notarization** — `release.yml`'s two macOS jobs and
  `signed-macos-canary.yml` use `block/apple-codesign-action`, a client for
  Block's internal codesigning service (`codesign_helper` Lambda + Buildkite)
  reached via an OIDC AWS role. The Apple certificate lives inside that Lambda,
  so **no secret a fork could supply makes it work.** Those jobs are left pinned
  to `block/buzz` so they skip instead of burning macOS runner time before
  failing. Use `macos-canary.yml` for an unsigned build; notarization needs
  either Block's #mdx-ios provisioning or a paid Apple Developer ID wired in
  directly.
- **macOS notarization** — and therefore a Gatekeeper-clean macOS build. See
  [Desktop auto-update](#desktop-auto-update-linux--windows--works) for what the
  fork ships instead, and what that costs a user.
- **Mobile release** — `mobile-release-candidate.yml` is dispatch-only, wants an
  exact `block/buzz` main SHA, and hands off to the private
  `squareup/sprout-releases` Buildkite pipeline.

### Desktop auto-update — works on all three platforms

Previously listed here as permanently broken. It was not: `assemble-manifest`
asserted `-ge 3` platforms, which a fork cannot reach with both macOS jobs
skipped, so `latest.json` was never published — while every updater archive and
`.sig` was already sitting in the `buzz-desktop-latest` rolling release. One
missing file, not a missing feature. The guard now asserts on job results
instead (see the `release.yml` row above).

The rest was already fork-correct: `BUZZ_UPDATER_ENDPOINT` points at this repo's
rolling release, the fork's own throwaway signing keypair produces the `.sig`
files, `desktop/scripts/generate-oss-latest-json.sh` hardcodes no platform, and
the app checks on mount and every 6 h
(`use-updater.ts:23`, `BACKGROUND_UPDATE_CHECK_INTERVAL_MS`).

`.deb` is not auto-updatable by Tauri constraint (`release.yml:699`), so Linux
auto-update is **AppImage only**.

#### macOS is unsigned, and users must be told what that means

`release-macos-unsigned` supplies `darwin-aarch64`. It mirrors the signed
`release` job's build, drops the `block/apple-codesign-action` step, and differs in
two more ways that are load-bearing rather than incidental: **no `--no-sign`**
(which would suppress updater signing as well as Apple signing, leaving the
`.app.tar.gz` without its `.sig`) and **`BUZZ_MACOS_ADHOC_SIGN=1`** on the config
step, which makes Tauri ad-hoc sign the bundle during bundling. So the app is
**ad-hoc signed and never notarized**.

That last sentence was aspirational until `fdf9c7a2e` (2026-08-01), and the gap is
worth understanding because nothing caught it for the lane's entire existence.
Without explicit `signingIdentity`, Tauri produced an `.app` with **no
`Contents/_CodeSignature` at all** — only the linker's ad-hoc signature on the
executable. A signed executable is not a signed bundle, and macOS reports the
difference as **"is damaged and can't be opened"**, which clearing the quarantine
attribute does not fix. Every macOS artifact this fork had produced, release and
canary alike, was unlaunchable; it went unnoticed because the DMGs were inspected
and never installed. An earlier version of this section asserted that the linker
signature "is what lets the result run on Apple Silicon at all" — it does not.

Ad-hoc signing must be **config, not a `codesign` step after the build**: Tauri
assembles the DMG inside the same `tauri build` invocation, so signing afterwards
fixes the `.app` on disk and leaves the DMG — the artifact users download —
carrying the unsigned copy.

**The diagnostic, if a macOS build ever misbehaves again:** `codesign -dv` reports
a signature in both the healthy and the broken case, so it cannot tell them apart —
look for `flags=0x20002(adhoc,linker-signed)`, which is the *broken* shape, versus
`0x2(adhoc)`. The check that actually distinguishes them is
`codesign --verify --deep --strict`, which both macOS lanes now run as a hard
assertion. They previously only *reported* it, with every command ending in
`|| true`; that is the direct reason a broken DMG reached a published release, and
it is the failure mode to watch for when adding any new artifact check.

To repair an already-downloaded artifact from before the fix (old canary artifacts
stay downloadable for ~90 days):

```
codesign --force --deep --sign - /Applications/BitcoinMarkets.app
xattr -dr com.apple.quarantine /Applications/BitcoinMarkets.app
```

Three consequences remain, none of which auto-update fixes:

1. **A first install still needs `xattr -dr com.apple.quarantine`.** Ad-hoc signing
   is not notarization, so Gatekeeper still objects — correctly now, as an
   unidentified developer rather than as damage. Auto-update does not remove that
   step; it stops it repeating, because later updates are written by the app itself
   and never carry a quarantine attribute.
2. **The ad-hoc cdhash changes every build**, so macOS may treat each update as a
   different program for keychain ACL purposes and re-prompt for, or lose, access
   to the stored identity. The app-data `identity.key` fallback is what carries
   the identity through — **unverified across an actual update**, and the first
   thing to check when testing one.
3. **The trust root is one throwaway key** plus write access to this repo's
   releases. No second opinion from Apple. Buying an Apple Developer ID and
   notarizing in this fork's own CI (Tauri supports it natively, no Block
   infrastructure) removes all three of these at once.

Two jobs can supply `darwin-aarch64`: signed `release` requires `block/buzz`,
unsigned requires *not* `block/buzz`. Complementary by construction, and
`assemble-manifest` asserts it rather than assuming — if both ever succeeded, the
second `write_sig` would overwrite the first and the manifest would pair one
lane's signature with the other's URL, which no client could verify and no later
step would catch.

`macos-canary.yml` stays deliberately non-updating (`createUpdaterArtifacts:
false`, no `BUZZ_UPDATER_*`, so `build.rs` leaves `buzz_updater_enabled` unset and
`lib.rs:339` never registers the plugin). Canaries are per-commit and have no
monotonic version for the updater to compare. **A canary build therefore cannot
update itself into the release channel** — moving from canary to auto-updating
requires installing a tagged release DMG once, by hand.

Known-still-hardcoded upstream, not yet patched here: `helm-chart.yml` has a
`GHCR_CHART_REPO` override but `push-gateway-helm-chart.yml` hardcodes
`CHART_REPO` (`:20`), and `signed-macos-canary.yml:104` still carries the
root-lockfile mesh-llm bug fixed in `ci.yml`.

---

## Ecosystem

Buzz spans five repos. This one (`block/buzz`) is the OSS source for the relay, desktop, mobile, and CLI. The others handle internal builds and deployment:

| Repo | Purpose |
|------|---------|
| [block/buzz](https://github.com/block/buzz) | OSS source — relay, desktop app, mobile app, CLI, agent harness |
| [squareup/sprout-releases](https://github.com/squareup/sprout-releases) | Buildkite pipeline producing Block-signed macOS + iOS builds with `-block` version suffix |
| [squareup/sprout-oss](https://github.com/squareup/sprout-oss) | CI pipeline building the relay Docker image and pushing to internal ECR |
| [squareup/block-coder-tf-stacks](https://github.com/squareup/block-coder-tf-stacks) | Terraform + ArgoCD deploying the relay to the staging Kubernetes cluster |
| [squareup/sprout-backend-blox](https://github.com/squareup/sprout-backend-blox) | Desktop backend provider script connecting Blox workstation agents to the relay |

```
block/buzz (source)
  ├─► sprout-releases    (desktop + mobile builds → Artifactory, GitHub, Mobile Releases)
  ├─► sprout-oss         (relay Docker image → ECR)
  │     └─► block-coder-tf-stacks  (Helm chart → ArgoCD → staging cluster)
  └─── sprout-backend-blox         (Blox compute provider for Desktop agent launch)
```

See [RELEASING.md](RELEASING.md) for the desktop release flow and
[CONTRIBUTING.md § Ecosystem](CONTRIBUTING.md#ecosystem) for contributor
access information.

---

## Repo Structure

```
crates/
  # Relay + core
  buzz-relay          # WebSocket relay server — main entry point; also hosts git + huddle audio
  buzz-core           # Core types, event verification, filter matching, kind registry
  buzz-db             # Postgres event store and data access layer
  buzz-auth           # Authentication and authorization
  buzz-pubsub         # Redis pub/sub fan-out, presence, typing indicators
  buzz-search         # Postgres FTS full-text search
  buzz-audit          # Hash-chain audit log
  buzz-media          # Blossom/S3 media storage
  # Agent surface
  buzz-acp            # ACP harness bridging Buzz events to AI agents
  buzz-agent          # Minimal ACP-compliant agent (non-streaming, tool-calls-as-output)
  buzz-dev-mcp        # Developer MCP server — shell + file-edit tools
  buzz-persona        # Agent persona packs
  buzz-workflow       # YAML-as-code workflow engine (evalexpr conditions)
  # Clients + interop
  buzz-paymaster      # FORK-LOCAL. Sponsors Starknet fees for Nostr-key accounts
  buzz-pair-relay     # Ephemeral sidecar relay for NIP-AB device pairing
  buzz-pairing-cli    # CLI for NIP-AB device pairing interop testing
  git-sign-nostr      # Sign git objects with a Nostr key
  git-credential-nostr # Git credential helper for Nostr-authed push/fetch
  # Tooling + shared
  buzz-cli            # Agent-first CLI
  buzz-sdk            # Typed Nostr event builders
  buzz-admin          # Operator CLI for relay administration
  buzz-ws-client      # Shared NIP-42 WebSocket client (connect, auth, publish)
  buzz-test-client    # Integration test client and E2E test suite
  sprig               # All-in-one harness bundling ACP, agent, and dev MCP

desktop/              # Tauri 2 + React 19 desktop app
web/                  # Browser web client (repo browser, served by the relay)
mobile/               # Flutter mobile app
migrations/           # SQL migrations (auto-applied on relay startup)
scripts/              # Dev tooling
.env.example          # Config template — copy to .env before running
```

---

## Getting Started

```bash
. ./bin/activate-hermit   # activate hermit toolchain (Rust, Node, etc.)
cp .env.example .env      # configure local environment
just setup                # install deps, run migrations
just relay                # start relay at ws://localhost:3000
just ci                   # run before any PR
```

See CONTRIBUTING.md for full setup details and dependency requirements.

---

## Quality Gates

Run `just ci` before every PR — it runs `fmt` + `clippy` + desktop lint +
unit tests + builds. Clippy passing does not mean fmt passes; run both.

Run `just test` for integration tests if you touched `buzz-relay`,
`buzz-db`, or `buzz-auth` — these require a running Postgres and Redis.

**Pre-commit hooks** are installed automatically by `just setup` and auto-fix
formatting via `stage_fixed`. Pre-commit runs fix variants in parallel (Rust
fmt, Tauri Rust fmt, desktop biome fix, web biome fix, mobile dart format).
Auto-fixable issues are fixed and re-staged; unfixable lint issues block the
commit. **Pre-push hooks** run clippy (workspace + Tauri) and fast unit tests
in parallel (Rust, desktop JS, Tauri Rust, mobile Flutter) — no overlap with
pre-commit. Builds are CI-only. Run `just fix-all` to auto-fix all formatting
in one shot. Run `just ci` for the full local gate. Run `just hooks` to
re-install hooks after env changes. Before agents run Git or hooks, activate the
repo's Hermit environment (`. ./bin/activate-hermit`); do not rewrite hook
commands to compensate for an unconfigured shell `PATH`.

**Commit with `git commit -s`.** The required **DCO Check** fails any PR with a commit missing a `Signed-off-by` trailer, and `just hooks` installs a `commit-msg` hook that adds it to commits you create locally (`git rebase` and `git cherry-pick` still need `--signoff`) — if you build commit commands programmatically, include `-s` every time. To repair a branch that already has unsigned commits: `git rebase --signoff main`, then force-push.

Additional rules:
- No `unsafe` code
- Do not introduce new `unwrap()` or `expect()` in production paths — use `?` and proper error types
- New public API must have doc comments

---

## Key Patterns

**Nostr-first HTTP surface**: Buzz's primary API is NIP-29 over WebSocket. The relay also exposes a narrow HTTP surface: NIP-11/NIP-05 metadata, `POST /events`, `POST /query`, `POST /count`, workflow webhooks at `/hooks/{id}`, Blossom media, git smart HTTP, git policy hooks, and health probes. These HTTP paths all preserve the same host-derived community boundary.

**Prefer Nostr events over new HTTP endpoints**: For new feature work, model
the operation as a Nostr event (new kind in `buzz-core/src/kind.rs`, handler
in `buzz-relay`) rather than adding endpoint-specific JSON APIs. HTTP is
reserved for things that genuinely need an HTTP-only surface: media upload/download
(Blossom), webhooks, git smart HTTP, NIP-11/NIP-05 metadata, health checks,
and the generic Nostr bridge endpoints:

- `POST /events` — submit any signed event (same path the WebSocket uses).
- `POST /query` — Nostr REQ filters over HTTP. NIP-50 `search` filters
  are routed to `buzz-search` (Postgres FTS) automatically.
- `POST /count` — Nostr COUNT filters over HTTP.

If you find yourself reaching for a new HTTP endpoint, first check whether
an event kind would do the job — it usually will, and you get realtime
fan-out, NIP-29 scoping, and the existing auth pipeline for free.

Reference https://github.com/nostr-protocol/nips

**Event kinds**: All event kind integers are defined in
`buzz-core/src/kind.rs`. New features get new kind integers — add them here
first, then implement handling in the relay.

**Channel scoping**: Channels use `h` tags (NIP-29 group tag), not `e` tags.
Filters and queries must scope to `h` tags when operating within a channel.
This applies to events *inside* a channel. Addressable events that describe a
channel carry its id in their `d` tag instead: kind:39000 (metadata),
kind:39001, kind:39002 (membership). `get_channels` resolves a user's channels
from the `d` tag of their kind:39002 events, not from `h`.

**Agent-facing operations go in `buzz-cli`**: New agent-facing features belong in `buzz-cli` — add a subcommand there first, then wire the REST/WebSocket call in `client.rs`. `buzz-dev-mcp` (shell + file tools for `buzz-agent`) is separate.

**Workflow conditions**: `buzz-workflow` uses
[evalexpr](https://docs.rs/evalexpr) for condition evaluation. Keep expressions
simple and testable.

**Thread counters**: `reply_count` and `descendant_count` are materialized on
thread root events. Any code that inserts replies must update these counters —
check existing reply handlers for the pattern.

---

## Agent CLI (`buzz-cli`)

`buzz` is the agent-first CLI. Auth env vars
(`BUZZ_RELAY_URL`, `BUZZ_PRIVATE_KEY`, `BUZZ_AUTH_TAG`) are auto-injected
by the ACP harness into managed agent subprocesses. In development, set
`BUZZ_PRIVATE_KEY` and `BUZZ_RELAY_URL` in your environment manually.

<!-- FORK-LOCAL NOTE (adrienlacombe/buzz) -->
> **Do not run the ACP harness with your personal key.** The harness runs *as*
> whatever `BUZZ_PRIVATE_KEY` it is given (`buzz-acp/src/config.rs:836`:
> `Keys::parse(&args.private_key)`), and forwards that same key into every managed
> agent subprocess (`buzz-acp/src/lib.rs:4198`). In production that key is the
> agent's own identity, so this is one principal handing its credential to its own
> tooling — not an escalation. The owner is a separate party, referenced only by
> pubkey via `BUZZ_ACP_AGENT_OWNER`.
>
> It matters in this fork because a Nostr key here also controls a Starknet account
> (`contracts/src/account.cairo`). Give the harness a dedicated agent key and it can
> spend only that agent's funds, which is the intended shape. Give it your own key
> and every agent subprocess can spend yours — and the key sits in plaintext in
> that subprocess's environment, readable from `/proc/<pid>/environ`, a crash dump,
> or any log that dumps env.

### Building the CLI

```bash
cargo build --release -p buzz-cli
```

Binary location: `./target/release/buzz`. Add `./target/release` to `PATH`
or invoke with the full path.

### Deep Links

`bitcoinmarkets://message?channel=<uuid>&id=<hex>` links reference a specific
message thread. This fork emits `bitcoinmarkets://` and still accepts `buzz://`,
so either scheme may turn up — older links in message history use the latter. To
read the linked thread:

```bash
buzz messages thread --channel <uuid> --event <hex> --format compact
```

Extract `channel` and `id` from the URL query parameters. The optional
`thread` parameter (root event ID) can be ignored — `messages thread` resolves
the full thread from the event ID alone.

All reads return sig-stripped JSON arrays; all writes return
`{event_id, accepted, message}`; creates add the entity ID. Exit codes:
0=ok, 1=input error, 2=network/relay, 3=auth, 4=other, 5=write conflict (NIP-33 LWW).

`--format compact` is a **global** flag — it goes before the subcommand:
`buzz --format compact channels list`, NOT `buzz channels list --format compact`.

See `crates/buzz-cli/TESTING.md` for the full live-testing runbook.

---

## Testing

```bash
just test-unit    # unit tests, no infrastructure needed
just test         # full integration suite (requires Postgres + Redis)
```

E2E tests live in `crates/buzz-test-client/tests/`:
- `e2e_relay.rs` — WebSocket relay protocol
- `e2e_media.rs` — media upload/download (Blossom)
- `e2e_media_extended.rs` — extended media scenarios
- `e2e_nostr_interop.rs` — Nostr interop (NIP-50 search, NIP-10 threads, NIP-17 gift wraps)

Desktop E2E: `cd desktop && pnpm exec playwright test`

See [TESTING.md](TESTING.md) for the full multi-agent E2E guide.

### PR Screenshots

> **Do NOT use `buzz upload`, the relay media endpoint, or any third-party
> image host for PR screenshots.** Relay media URLs fail through GitHub's camo
> proxy. Always use `scripts/post-screenshots.sh` for PNGs before linking them
> from a PR body/comment. If you hand-edit PR markdown, run
> `scripts/check-pr-image-urls.sh <markdown-file>` first to catch relay URLs.

For mobile simulator screenshots, save the PNGs in a local directory and run
`./scripts/post-screenshots.sh <PR-number> <png-dir>` or use the third argument
with a markdown template containing `{{filename}}` placeholders.

The desktop app requires the E2E mock bridge to render — it cannot run in a plain
browser. Use `just desktop-screenshot` to capture screenshots (builds frontend,
starts preview server, runs Playwright automatically):

```bash
just desktop-screenshot --name home
just desktop-screenshot --name channel --route /channels/general
just desktop-screenshot --name search --click open-search
just desktop-screenshot --name settings --click open-settings
```

Options: `--name` (filename), `--route` (client route), `--active-channel`
(channel to view), `--click` (left-click data-testid or CSS selector),
`--right-click` (right-click for context menus), `--hover` (hover before
capture), `--clip` (crop region as `x,y,w,h` — e.g. `0,0,256,720` for sidebar
only), `--wait` (ms, default 2000), `--viewport` (WxH, default 1280x720),
`--outdir` (default `test-results/screenshots`), `--messages` (JSON file path).
Output is a PNG path on stdout.

Use `--messages` to inject content into a channel before capture. The JSON file
is an array of objects — `channelName` and `content` are required, all other
fields are optional and passed through to `__BUZZ_E2E_EMIT_MOCK_MESSAGE__`:

```json
[
  {
    "channelName": "random",
    "content": "Hey @tyler check this out",
    "pubkey": "953d...",
    "kind": 40002,
    "mentionPubkeys": ["deadbeef..."],
    "extraTags": [["broadcast", "1"], ["e", "some-root-id"]],
    "parentEventId": "abc123"
  }
]
```

Without `--active-channel`, all messages must target the same channel and the
helper navigates to that channel (useful for showing message content). With
`--active-channel`, messages can target multiple channels while the "camera"
stays on the specified channel (useful for unread indicators, badges, etc.).

```bash
# Messages in the channel you're viewing (code blocks, formatting, etc.)
just desktop-screenshot --name code-blocks --messages /tmp/msgs.json

# Messages in OTHER channels to trigger unread state
just desktop-screenshot --name unread-dot \
  --active-channel general --messages /tmp/badge-msgs.json

# Cropped to sidebar only (256px wide)
just desktop-screenshot --name sidebar-unread \
  --active-channel general --messages /tmp/badge-msgs.json \
  --clip 0,0,256,720

# Context menu on an unread channel (wider crop to include popup)
just desktop-screenshot --name ctx-mark-read \
  --active-channel general --messages /tmp/badge-msgs.json \
  --right-click channel-random --clip 0,200,320,300

# Hover state (e.g. copy button reveal)
just desktop-screenshot --name copy-hover \
  --messages /tmp/code-msgs.json --hover "[data-testid='copy-code']"
```

Available mock channels: `general`, `random`, `design`, `sales`, `engineering`,
`agents`, `watercooler`, `announcements`, `alice-tyler`, `bob-tyler`.

`scripts/post-screenshots.sh` hosts PNGs on a per-developer branch
(`agent-screenshots/<github-username>`) and posts a PR comment with
commit-SHA-based image URLs (immutable — safe from later overwrites):

```bash
./scripts/post-screenshots.sh 803 test-results/screenshots
./scripts/post-screenshots.sh 803 test-results/screenshots body.md  # custom body prepended
```

The body file supports `{{filename}}` placeholders (without `.png`) to inline
images at specific positions. Images not referenced by any placeholder are
appended at the end. Without placeholders, all images are appended (backward
compatible).

```markdown
### Unread dot
A message arrives in `#random`.

{{01-unread-dot}}

### Context menu
Right-click shows "Mark as read".

{{02-context-menu}}
```

Re-runs overwrite the image blobs on the `agent-screenshots/<username>`
branch, but the script **appends a new PR comment** — it does not edit or
delete the previous one. After reposting, delete the superseded comment so
only the current set remains, otherwise reviewers still see the stale images:

```bash
# List screenshot comments to find the stale one's id
gh pr view <pr> --repo block/buzz --json comments \
  --jq '.comments[] | select(.body | test("pr-<pr>--")) | {id, url}'
gh api -X DELETE repos/block/buzz/issues/comments/<stale-comment-id>
```

Branch cleanup when fully done: `git push origin --delete agent-screenshots/<username>`.

### Writing E2E Screenshot Specs

When screenshots need seeded state, live messages, or UI interaction before
capture, write a Playwright spec instead of using `just desktop-screenshot`.
Add specs to `desktop/tests/e2e/` and register them in `playwright.config.ts`
(`smoke` project `testMatch`). Every test calls `installMockBridge(page)` for
mock Tauri IPC. Mock pubkey, channel names, and UUIDs live in `e2eBridge.ts`.

**Always build with `pnpm build:e2e`, never `pnpm run build`.** The mock Tauri
bridge is compiled in only for `--mode e2e` (see `installE2eBridgeIfConfigured`
in `desktop/src/main.tsx`). A plain `pnpm run build` strips it, so
`window.__TAURI_INTERNALS__` is never defined and **every** mock-mode spec fails
with `Cannot read properties of undefined (reading 'invoke')` — the app renders
"Community connection failed" instead of the UI under test. That looks exactly
like a product bug rather than a build mistake, so it burns real time.
`pnpm test:e2e:smoke` and `pnpm test:e2e:integration` run the right build for
you; prefer them over a manual build plus `playwright test`.

**Stale server:** `reuseExistingServer: true` means a previous build's server
serves old code. Kill port 4173 and re-run `pnpm build:e2e` before re-running
tests after code changes.

**`addInitScript` before bridge:** `page.addInitScript` (localStorage seeding)
must run BEFORE `installMockBridge(page)` — React reads state on mount, the
bridge triggers mount.

**Live messages:** Call `waitForMockLiveSubscription(page, channelName)` before
`__BUZZ_E2E_EMIT_MOCK_MESSAGE__` — messages are silently dropped without a
subscription. Navigate to the channel first (triggers subscription), then away
(so unread indicators appear), then inject.

**Animation timing:** Radix components animate in via CSS. `toBeVisible()`
resolves mid-animation — wait for completion before screenshotting. Use the
shared helper (mandatory before any `page.screenshot()` or
`locator.screenshot()` in specs):

```ts
import { waitForAnimations } from "../helpers/animations";

// ... after the element is visible but before capturing:
await waitForAnimations(page);
await page.screenshot({ path: "...", clip: { ... } });
```

The `just desktop-screenshot` path (`screenshot.mjs`) calls
`waitForAnimations` automatically — no manual step needed there.

For per-element waits (rare — prefer the page-level helper above):

```ts
await menuItem.evaluate((el) =>
  Promise.all(
    el.closest("[data-state]")?.getAnimations().map((a) => a.finished) ?? [],
  ),
);
```

**Cropping:** Use `clip` — full-window (1280x720) screenshots are unreadable
for sidebar features. Sidebar = 256px; context menus ~450px.

**Distinct states — verify before posting:** when one view renders many
elements at once (e.g. all team cards in a single grid), an unscoped
full-page `page.screenshot()` captures the *same* pixels for every shot, so
multiple PNGs come out byte-identical. Scope each shot to its subject with
`locator.screenshot()` (full-page `clip` only when an overlay like an open
dropdown must be included). Then gate on hash distinctness before posting:

```bash
shasum -a 256 test-results/<dir>/*.png   # every hash must be unique
```

Identical hashes mean two shots captured the same state — fix the spec, do
not post. This catches the most common screenshot regression.

**`general` has pre-seeded messages** making `hasUnread` always true. Use
`engineering` for "muted + no unread" visual states.

**PR comments:** Use a body template (3rd arg to `post-screenshots.sh`) with
`{{filename}}` placeholders. Each screenshot gets a `###` heading + one-line
description. See [PR #803](https://github.com/block/buzz/pull/803).

---

## Common Gotchas

1. **Kind `39000` for channel metadata, not `41`** — kind 41 is NIP-01 (unused). All kinds defined in `buzz-core/src/kind.rs`.
2. **Relay queries must specify `kinds`** — omitting `kinds` triggers the p-gate (403). Always include explicit kind filters.
3. **`messages search` must include `--kinds`** — an open-ended search (no kinds) hits the relay p-gate and returns 403. Pass at least `--kinds 9,45001,45003` to scope the query.
4. **Worktrees: `cd` in the same command** — shell CWD doesn't persist between tool calls. Use `cd /path && cargo build` as one command.
5. **Desktop crate excluded from root workspace** — `cargo test` at repo root does NOT run desktop tests. Use `cargo test --manifest-path desktop/src-tauri/Cargo.toml` explicitly.
6. **Desktop Tauri fmt fails in worktrees and blocks commits** — the pre-commit hook runs `just desktop-tauri-fmt`, which fails in git worktrees because `cargo fmt` resolves workspace paths relative to the worktree root. Run `just desktop-tauri-fmt` from the main checkout to apply the fix, then re-stage and commit. CI is unaffected.
7. **React render perf: `React.memo` is all-or-nothing** — it only skips a re-render when *every* prop is reference-stable; one unstable prop (inline arrow/JSX, or a hook returning a fresh `{}`/`[]`/`Map` each render) defeats it. Two repeat offenders: (a) React Query results (`useMutation`/`useQuery`) are a **new object each render** — depend on the stable method (`mutation.mutateAsync`), not the object; (b) derived `Map`/array state that recomputes on a version bump — wrap in a content-equality ref cache (`shared/hooks/useStableReference.ts`). When chasing interaction lag, **measure with DevTools closed and no perf probes** (an open Web Inspector + per-keystroke `console.log` inflate the numbers), and isolate by removing one suspect at a time rather than guessing.

---

## Desktop App

The desktop app is Tauri 2 + React 19 + Vite + Tailwind CSS. Features are
organized under `desktop/src/features/`. Biome handles linting and formatting.

```bash
just desktop-dev   # web-only dev server (faster iteration)
just dev           # full Tauri app with native shell
```

### Text sizing & zoom (use rem, never px)

The desktop app implements Cmd +/- zoom by scaling the root `<html>`
font-size (`desktop/src/app/useWebviewZoomShortcuts.ts`) and pinning the native
webview zoom. **Only rem-based text scales with zoom — hardcoded px text sizes
are frozen.**

So for any readable text, reach for rem-based Tailwind tokens, never arbitrary
px:

- ✅ Stock rem tokens (`text-base`, `text-sm`, `text-xs`, …). **Chat body/author
  text === `text-base` (16px) — chat is the app's base type size**, and the
  surrounding timeline elements (timestamps, system rows, code, reactions) are
  deliberate steps on that same stock ramp.
- ✅ The `text-2xs` (0.6875rem / 11px) and `text-3xs` (0.5rem / 8px) meta-text
  tokens (in `desktop/tailwind.config.js` under `theme.extend.fontSize`) for the
  sub-`text-xs` ramp — timestamps, count badges, tracking labels, tiny glyphs.
  These replaced the dozens of arbitrary `text-[…rem]` literals that had drifted
  apart pixel-by-pixel; keep meta text on these two tokens, not new arbitrary
  values.
- ❌ `text-[15px]`, `text-[13px]`, CSS `font-size: 15px` — px froze against zoom
  and caused the message-timeline regression (PR #891).
- ❌ Arbitrary rem literals too: `text-[0.6875rem]`, `text-[0.9rem]`, etc. They
  zoom fine but re-fragment the scale we consolidated. Use a named token.

Prefer stock tokens — they're rem and zoom-safe. Only if a design genuinely
needs a size the stock/`2xs`/`3xs` scale can't express should you **add a
rem-based token** (in `desktop/tailwind.config.js` under `theme.extend.fontSize`)
rather than an arbitrary literal. A CI guard (`pnpm check:px-text`, in
`desktop/scripts/check-px-text.mjs`) scans all of `desktop/src` and fails on any
new arbitrary text-size literal — px **or** rem/em. Genuinely decorative glyphs
(e.g. the `text-[6rem]` avatar emoji) are allowlisted by `path:line` in that
script.

### Community Switching

The desktop app supports multiple communities (each backed by a different relay).
Switching communities does **not** reload the page — it uses React key-based
remounting. `<AppReady key={communityKey} />` in `App.tsx` forces the entire
community-scoped subtree to unmount and remount with fresh state.

**Module-level singletons must be explicitly reset.** React remounting only
clears React state (useState, useRef, context). Module-level variables (Maps,
class instances, cached promises) survive across remounts. Every community-scoped
singleton needs a reset function wired into `resetCommunityState()` in
`desktop/src/features/communities/useCommunityInit.ts`.

Current singletons that are reset on relay boundary changes (same-relay
reconnects preserve pending avatar verification work):
- `relayClient.disconnect()` — WebSocket teardown + promise rejection
- `resetRateLimitGate()` — clears any active rate-limit window from the old relay
- `clearAllDrafts()` — message draft cache
- `resetAgentObserverStore()` — agent observer relay store
- `resetActiveAgentTurnsStore()` — active agent turn timers
- `resetAgentWorkingSignal()` — agent working indicator signal
- `resetAvatarProfileSync()` — pending verified-avatar profile writes
- `resetAvatarPresentations()` — avatar probes, previews, and Retry toasts
- `resetSidebarRelayConnectionCardState()` — sidebar relay card dismiss state
- `resetMediaCaches()` — proxy port and relay origin caches
- `resetVideoPlayerState()` — video player singleton
- `resetRenderScopedReactionHydration()` — reaction hydration cache
- `clearSearchHitEventCache()` — search result event cache
- `clearMarkdownNodeCache()` — markdown parse-node cache

**If you add a new module-level cache, Map, or class instance that holds
community-scoped data, you must add its reset to `resetCommunityState()`.**
Failure to do so causes data from the old community to leak into the new one.

Key files:
- `desktop/src/app/App.tsx` — community key, init gate, remount boundary
- `desktop/src/features/communities/useCommunityInit.ts` — `resetCommunityState()`, applies config to Tauri backend
- `desktop/src/main.tsx` — provider hierarchy (`QueryClientProvider` > `App`)

---

## Mobile App (Flutter)

The mobile app lives in `mobile/` — a Flutter app using Riverpod + Hooks.

### Architecture

- **State management:** Riverpod + `flutter_hooks` (`HookConsumerWidget`)
- **Theme:** Catppuccin Latte (light) / Macchiato (dark) — matches desktop
- **Features:** Isolated under `lib/features/`, shared code in `lib/shared/`
- **Nostr models:** `lib/shared/relay/nostr_models.dart` — event kinds must
  stay in sync with `desktop/src/shared/constants/kinds.ts`

### Rules

- **NEVER use `StatefulWidget`** — favor Riverpod for state and always use
  `HookConsumerWidget` or `ConsumerWidget` with `flutter_hooks` for local state.
- **NEVER run `flutter run`, `flutter build`, `flutter clean`, or
  `flutter upgrade`** — only `flutter test`, `flutter analyze`, and
  `dart format` are safe for agents to run.
- **Do NOT use `print()`** — use `debugPrint()` or structured logging.
- Prefer `context.colors` and `context.textTheme` (via theme extensions)
  over raw `Theme.of(context)` calls.
- **Keep widgets small and composable.** One public widget per file; push
  private sub-widgets (`_Foo`) into sibling `part` files under a
  `<page>/` folder rather than growing the page file. Hard ceiling:
  **1000 lines/file**, enforced by `mobile/scripts/check-file-sizes.mjs` via
  `just mobile-check` (runs in `just check` + pre-push, mirroring desktop/web).
  If the guard trips, **split the file — never bump the limit or add an
  override to slip under it.**
- Feature modules must not import from other feature modules — only from
  `shared/`.
- Use `Grid` tokens for spacing, `Radii` for border radius.

### Quality Checks

```bash
cd mobile
dart format --output=none --set-exit-if-changed .
flutter analyze
flutter test
```

Or from repo root: `just mobile-fmt` (auto-fix), `just mobile-check` (lint + fmt check), `just mobile-test` (tests).

To run the app locally (starts Docker, relay, iOS simulator automatically):

```bash
just mobile-dev
```

When run from a git worktree, `just mobile-dev` (and `just
mobile-build-android`) give the debug build a per-worktree app identifier
(keyed to the worktree directory name) and a branch-labelled app name via
`scripts/mobile-worktree-overrides.sh`, so builds from multiple worktrees
install side by side. Release builds are unaffected. `just mobile-clean`
removes stale worktree-suffixed installs from simulators/emulators. See
[mobile/README.md](mobile/README.md) for direct Xcode / Android Studio
usage.

### Testing Conventions

- Prefer **widget tests** over unit tests for UI components — test the
  whole widget tree, not individual methods.
- Use `ProviderScope(overrides: [...])` to inject fake notifiers.
- Fake notifiers should extend the real notifier class and override `build()`.
- Use the `WidgetHelpers.testable()` wrapper for simple widget tests or
  build a custom `ProviderScope` + `MaterialApp` when you need specific overrides.

---

## See Also

- [CONTRIBUTING.md](CONTRIBUTING.md) — setup, code style, PR process, how to add event kinds / CLI subcommands / HTTP endpoints
- [TESTING.md](TESTING.md) — multi-agent E2E test guide
- [ARCHITECTURE.md](ARCHITECTURE.md) — system design and component relationships
- [RELEASING.md](RELEASING.md) — release process: `release-desktop`, `release-relay`, `scripts/mobile-release.sh`, candidate tags, internal builds
- [README.md](README.md) — project overview and quick start
