---
on:
  schedule:
    # 02:00 UTC every day
    - cron: "0 2 * * *"
  workflow_dispatch:

engine: copilot

permissions:
  contents: read

timeout-minutes: 20

network:
  allowed:
    - defaults

tools:
  # git only — the merge happens in the checkout, and the pull request / issue
  # are created by the safe-outputs job, not by the agent calling the API.
  bash:
    - "git"

safe-outputs:
  create-pull-request:
    title-prefix: "[upstream-sync] "
    labels: [upstream-sync, automation]
    if-no-changes: "ignore"
    # An upstream sync legitimately carries changes to .github/, README.md,
    # package manifests, etc. Without this, every sync PR gets an automatic
    # REQUEST_CHANGES review for touching protected paths.
    protected-files: allowed
    max: 1
    # gh-aw's defaults (100 files / 4096 KB) are sized for a small agent-authored
    # patch. An upstream sync is a `git merge` of upstream's own commits, so its
    # size is set by upstream's activity, not by anything the agent decides — and
    # the result is reviewed as a PR before it lands. Sizing these to "whatever
    # upstream produced" is therefore the correct trade, not a loosened guardrail.
    #
    # Measured on the first real sync: 14 commits = 136 files / 926 KB. The
    # 100-file default blocked the PR outright and the run reported E003 instead
    # (issue #4) — every non-trivial sync would have failed the same way. At
    # ~10 files and ~66 KB per commit, a week of drift lands near 700 files and
    # 6.6 MB, which is why the size cap is raised too rather than only the count.
    #
    # 10240 KB is gh-aw's schema maximum, not a chosen value — roughly 155 commits
    # of headroom at the rate measured above, so about two weeks of drift. If the
    # sync is ever down longer than that it will fail on size with no knob left,
    # and the merge has to be done by hand.
    # Requests GitHub Copilot's code review on the sync PR. This is the only
    # AI reviewer GitHub exposes as a requestable reviewer identity — there is no
    # "Opus" reviewer to request. gh-aw's own engine currently runs on
    # claude-sonnet-4.6, and pinning engine.model changes who *writes* the merge,
    # not who reviews it; gh-aw passes that string through unvalidated, so a wrong
    # value fails the run at inference time.
    # gh-aw accepts this, but it did not attach on PR #7 — the requested-reviewers
    # list came back empty and had to be set by API with the bot's real login,
    # copilot-pull-request-reviewer[bot]. Kept because it is the documented value
    # and harmless, but upstream-sync-ci-status.yml requests the reviewer directly
    # so it does not depend on this working.
    reviewers: [copilot]
    # gh-aw opens drafts by default. A draft cannot be merged and does not get a
    # Copilot review, so the sync PR would sit unreviewable.
    draft: false
    max-patch-files: 2000
    max-patch-size: 10240
  create-issue:
    title-prefix: "[upstream-sync] "
    labels: [upstream-sync, automation]
    max: 1
---

# Sync this fork with upstream

`${{ github.repository }}` is a fork of the upstream repository **`block/buzz`**.
Your job is to bring this fork's `main` branch up to date with upstream `main`,
resolving merge conflicts if there are any, and to propose the result as a pull
request. You never push to `main` yourself.

## 1. Fetch upstream

```
git remote add upstream https://github.com/block/buzz.git
git fetch --unshallow origin
git fetch upstream main
```

The first command fails harmlessly if the remote already exists; the second
fails harmlessly if the checkout is already complete. Ignore both errors and
carry on.

## 2. Decide whether there is anything to do

```
git rev-list --left-right --count upstream/main...HEAD
```

The left number is how many commits upstream has that this fork does not; the
right number is how many commits this fork has that upstream does not.

If the left number is **0**, the fork is already current. Say so and stop —
do not create a pull request, and do not create an issue. This is the expected
outcome on most days.

## 3. Merge upstream

```
git merge upstream/main
```

Keep the merge commit — do **not** squash and do **not** rebase, so that
upstream's individual commits stay in the history and the next run can tell
what is already merged.

If the merge completes cleanly, go to step 5.

## 4. Resolve conflicts, if any

If the merge stops with conflicts:

- List them with `git status` and inspect each one with `git diff`.
- Resolve each conflict on its merits. Take upstream's version for files this
  fork has not deliberately customized. Preserve this fork's version where the
  local change is clearly intentional. When both sides changed different parts
  of the same file, keep both.
- Do not delete a file to resolve a conflict unless upstream deleted it.
- Do not reformat, refactor, upgrade dependencies, or "fix" anything unrelated
  to the conflict. The diff must contain nothing but upstream's changes plus
  your conflict resolutions.
- Finish with `git add <files>` and `git commit --no-edit`.

**If you cannot resolve a conflict with confidence** — the two sides make
contradictory design decisions, or resolving it correctly needs context you do
not have — stop merging, run `git merge --abort`, and create an issue instead
of a pull request. The issue should name the conflicting files, explain what
each side is doing, and state what a human needs to decide. Do not guess.

## 5. Open the pull request

Create a pull request from the merge. Write a body that covers:

- The upstream commit range that is being merged, and how many commits it is.
- A short summary of what changed upstream, grouped by area (relay, desktop,
  mobile, CLI, CI, docs — see `CLAUDE.md` for the repo layout).
- **Conflicts:** every file that conflicted and, in one line each, how you
  resolved it. Say "None — clean merge" if there were none.
- **Needs a human look:** anything you were unsure about, anything that touches
  `.github/`, and any upstream change that plausibly interacts with a local
  customization of this fork.

Do not run builds, tests, linters, or `just ci` — CI on the pull request does
that. Your only job is the merge and an honest description of it.
