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
