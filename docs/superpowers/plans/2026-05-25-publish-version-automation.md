# Publish Version Automation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the manual version-gated `publish.yml` with a release-plz-driven release-PR workflow that auto-bumps per-crate versions based on conventional commits, generates per-crate changelogs, and publishes to crates.io after human review.

**Architecture:** Per-crate versions (break `workspace.package.version`). Two GitHub Actions workflows: `release-plz-pr.yml` (opens/amends "chore: release" PR on every push to `main`) + `release-plz-release.yml` (publishes to crates.io when a release commit lands on `main`). `cargo-semver-checks` enforces API-break detection. Migration runs in 5 phases with dry-run validation before live cutover; old `publish.yml` is deleted only at the final phase.

**Tech Stack:** release-plz v0.5.129, cargo-semver-checks, conventional-commits, GitHub Actions, Rust 1.85 / Edition 2024.

**Spec:** [`docs/superpowers/specs/2026-05-25-publish-version-automation-design.md`](../specs/2026-05-25-publish-version-automation-design.md)

---

## Phase 0: Land pending path-only dev-dep fix

The previously-committed fix (`49ececc`) for `zendriver-mcp`'s `zendriver-interception` dev-dep is sitting on local `main` unpushed because the auto-mode classifier blocked the direct push. This unblocks today's publish and is **independent** of the release-plz work — but every later phase assumes it has landed, so we settle it first.

### Task 0.1: Land commit `49ececc` on origin/main

**Files:**
- Already-committed: `crates/zendriver-mcp/Cargo.toml` (the path-only dev-dep change)

- [ ] **Step 1: Confirm commit is still on local main**

```bash
git -C /Users/rin/GitHub/zendriver-rs log origin/main..main --oneline
```

Expected: shows `49ececc fix(zendriver-mcp): make zendriver-interception dev-dep path-only` (and possibly `303eced docs: spec for publish version automation via release-plz`).

- [ ] **Step 2: Open a PR for the fix (do NOT push to main directly)**

```bash
git -C /Users/rin/GitHub/zendriver-rs push origin main:fix/zendriver-mcp-publish-dev-dep
gh pr create --base main --head fix/zendriver-mcp-publish-dev-dep \
  --title "fix(zendriver-mcp): make zendriver-interception dev-dep path-only" \
  --body "$(cat <<'EOF'
## Summary
- Switch `zendriver-mcp`'s `zendriver-interception` dev-dep to path-only (no version) so `cargo publish` strips it from the published manifest.
- Unblocks the `Publish` workflow, which was failing because crates.io's `zendriver-interception` 0.1.0 lacks the `test-support` feature the dev-dep was requesting.

## Test plan
- [x] `cargo publish --dry-run --locked --allow-dirty --manifest-path crates/zendriver-mcp/Cargo.toml` succeeds locally (verified pre-commit)
- [ ] CI green on this PR
- [ ] After merge: `Publish` workflow on main succeeds and uploads `zendriver-mcp v0.1.0`
EOF
)"
```

Expected: PR opened. CI runs.

- [ ] **Step 3: Wait for CI green, then merge**

```bash
gh pr checks fix/zendriver-mcp-publish-dev-dep
gh pr merge fix/zendriver-mcp-publish-dev-dep --squash --delete-branch
```

Expected: CI green, merged via squash, branch deleted.

- [ ] **Step 4: Confirm Publish workflow succeeds on main**

```bash
git -C /Users/rin/GitHub/zendriver-rs pull --ff-only
sleep 90   # let Publish workflow start + finish
gh run list --workflow=publish.yml --limit 1
```

Expected: most recent `Publish` run shows `success`. `zendriver-mcp v0.1.0` now on crates.io.

- [ ] **Step 5: No additional commit needed** — Phase 0 is purely about landing existing work.

---

## Phase 1: Per-crate versions (split `workspace.package.version`)

Splitting `workspace.package.version` is a workspace-wide refactor with no behavior change. Each crate keeps `0.1.0`; release-plz will start bumping them independently in later phases.

### Task 1.1: Replace `version.workspace = true` with explicit `version = "0.1.0"` per crate

**Files:**
- Modify: `Cargo.toml` (workspace root — remove `version = "0.1.0"` from `[workspace.package]`)
- Modify: `crates/zendriver-transport/Cargo.toml`
- Modify: `crates/zendriver-stealth/Cargo.toml`
- Modify: `crates/zendriver-interception/Cargo.toml`
- Modify: `crates/zendriver-fetcher/Cargo.toml`
- Modify: `crates/zendriver-cloudflare/Cargo.toml`
- Modify: `crates/zendriver/Cargo.toml`
- Modify: `crates/zendriver-mcp/Cargo.toml`

- [ ] **Step 1: Remove `version` from workspace.package**

In `Cargo.toml` at the workspace root, find `[workspace.package]` and delete the line `version = "0.1.0"`. Keep all other fields (`edition`, `rust-version`, `license`, `repository`, `authors`).

Resulting block:

```toml
[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT OR Apache-2.0"
repository = "https://github.com/TurtIeSocks/zendriver-rs"
authors = ["zendriver-rs contributors"]
```

- [ ] **Step 2: Replace `version.workspace = true` with `version = "0.1.0"` in each crate**

For each of the 7 crate `Cargo.toml` files listed under **Files**, find:

```toml
version.workspace = true
```

Replace with:

```toml
version = "0.1.0"
```

Leave every other workspace inheritance line untouched (`edition.workspace`, `rust-version.workspace`, etc.).

- [ ] **Step 3: Run `cargo metadata` to verify versions resolve**

```bash
cd /Users/rin/GitHub/zendriver-rs && cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | "\(.name) \(.version)"' | sort
```

Expected output:

```
zendriver 0.1.0
zendriver-cloudflare 0.1.0
zendriver-fetcher 0.1.0
zendriver-interception 0.1.0
zendriver-mcp 0.1.0
zendriver-stealth 0.1.0
zendriver-transport 0.1.0
```

- [ ] **Step 4: Run full workspace build to confirm no regressions**

```bash
cd /Users/rin/GitHub/zendriver-rs && cargo check --workspace --all-features
```

Expected: succeeds. No warnings introduced by the change.

- [ ] **Step 5: Commit**

```bash
git -C /Users/rin/GitHub/zendriver-rs add Cargo.toml crates/*/Cargo.toml
git -C /Users/rin/GitHub/zendriver-rs commit -m "$(cat <<'EOF'
refactor(workspace): split workspace.package.version into per-crate versions

Replaces `version.workspace = true` in each crate's Cargo.toml with an
explicit `version = "0.1.0"` and removes `version` from
[workspace.package] in the root Cargo.toml.

No behavior change — all crates still at 0.1.0. This is the prerequisite
for release-plz, which needs each crate's version to move independently
based on per-crate file changes.

Spec: docs/superpowers/specs/2026-05-25-publish-version-automation-design.md
EOF
)"
```

### Task 1.2: Verify `[workspace.dependencies]` internal pins still match

**Files:**
- Already correct: `Cargo.toml` (workspace root — `[workspace.dependencies]` already has explicit `version = "0.1.0"` pins per internal crate; no edit needed)

- [ ] **Step 1: Confirm internal deps have explicit version pins**

```bash
grep -E 'zendriver(-[a-z]+)? *=' /Users/rin/GitHub/zendriver-rs/Cargo.toml | grep 'version'
```

Expected: seven lines, each with `version = "0.1.0"`.

- [ ] **Step 2: No commit if no change** — this is a verification-only task. If a pin is missing the version field, add `version = "0.1.0"` and amend the previous commit.

### Task 1.3: Open Phase-1 PR

- [ ] **Step 1: Push branch and open PR**

```bash
git -C /Users/rin/GitHub/zendriver-rs push origin main:chore/per-crate-versions
gh pr create --base main --head chore/per-crate-versions \
  --title "refactor(workspace): split workspace.package.version into per-crate versions" \
  --body "$(cat <<'EOF'
## Summary
- Each crate now owns its own `version` field instead of inheriting from `[workspace.package]`.
- Prereq for the release-plz migration (see [spec](docs/superpowers/specs/2026-05-25-publish-version-automation-design.md)).
- No behavior change — every crate is still at 0.1.0.

## Test plan
- [x] `cargo metadata` reports 0.1.0 for all 7 crates
- [x] `cargo check --workspace --all-features` passes locally
- [ ] CI green
EOF
)"
```

- [ ] **Step 2: Wait for CI, merge**

```bash
gh pr checks chore/per-crate-versions
gh pr merge chore/per-crate-versions --squash --delete-branch
git -C /Users/rin/GitHub/zendriver-rs checkout main && git -C /Users/rin/GitHub/zendriver-rs pull --ff-only
```

Expected: CI green, merged. **Note:** the existing `publish.yml` will fire on this push and should report "no crates needed publishing" (versions match crates.io for all 7).

- [ ] **Step 3: Confirm `publish.yml` did not republish anything**

```bash
sleep 60
gh run list --workflow=publish.yml --limit 1
```

Expected: `success`, summary shows all crates skipped.

---

## Phase 2: Add release-plz configuration files

Add `release-plz.toml` and the changelog template at the workspace root. No workflows yet — config only.

### Task 2.1: Write `release-plz.toml`

**Files:**
- Create: `release-plz.toml`

- [ ] **Step 1: Create the config file**

```toml
# release-plz workspace configuration.
# Docs: https://release-plz.ieni.dev/docs/config

[workspace]
# Run cargo-semver-checks before bumping. Catches API breaks that
# conventional commits forgot to mark with `!`.
semver_check = true

# Generate per-crate CHANGELOG.md. The top-level CHANGELOG.md stays
# human-curated (release-plz does not touch the workspace root).
changelog_update = true

# Open a GitHub Release for each published crate version, with the
# changelog snippet as the release body.
git_release_enable = true

# Use conventional commits for bump magnitude inference. The template
# below also drives the generated CHANGELOG entries.
changelog_config = "release-plz-changelog.toml"

# Don't release a crate just because its files changed — require a
# conventional commit that maps to a bump (fix/feat/BREAKING). This is
# the safer default; flip to `true` per-crate if we lose releases.
# release_always = false  # ← default

# Don't skip the cargo-publish verify step.
publish_no_verify = false

# Disable changelog generation for purely-internal crates? — none here,
# all 7 are public.

# zendriver-mcp is a binary crate. release-plz still publishes it so
# `cargo install zendriver-mcp` works.
[[package]]
name = "zendriver-mcp"
publish = true
```

- [ ] **Step 2: Validate TOML syntax**

```bash
cd /Users/rin/GitHub/zendriver-rs && cargo install --locked release-plz 2>/dev/null || true
release-plz config check 2>&1 || echo "(release-plz not yet installed locally — config will be validated by the GitHub Action)"
```

Expected: either "config OK" or the install-skip note. If `release-plz` is installed and reports an error, fix and re-run.

### Task 2.2: Write `release-plz-changelog.toml`

**Files:**
- Create: `release-plz-changelog.toml`

- [ ] **Step 1: Create the changelog template**

This uses [git-cliff](https://git-cliff.org/docs/configuration) format under the hood. The template intentionally mirrors the project's existing [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) style.

```toml
# release-plz changelog template — extends git-cliff config.
# Docs: https://git-cliff.org/docs/configuration

[changelog]
header = """
# Changelog

All notable changes to this crate documented here. Format: [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/). Adheres to [SemVer](https://semver.org/).

"""
body = """
{% if version %}\
    ## [{{ version | trim_start_matches(pat="v") }}] - {{ timestamp | date(format="%Y-%m-%d") }}
{% else %}\
    ## [unreleased]
{% endif %}\
{% for group, commits in commits | group_by(attribute="group") %}
    ### {{ group | upper_first }}
    {% for commit in commits %}
        - {{ commit.message | upper_first | trim }}\
            {% if commit.breaking %} **(BREAKING)**{% endif %}\
    {% endfor %}
{% endfor %}\n
"""
trim = true

[git]
conventional_commits = true
filter_unconventional = true
commit_parsers = [
    { message = "^feat", group = "Added" },
    { message = "^fix", group = "Fixed" },
    { message = "^perf", group = "Performance" },
    { message = "^refactor", group = "Changed" },
    { message = "^docs", skip = true },
    { message = "^test", skip = true },
    { message = "^ci", skip = true },
    { message = "^chore", skip = true },
    { message = "^style", skip = true },
]
filter_commits = true
tag_pattern = "v[0-9].*"
```

- [ ] **Step 2: No test-run needed in this task** — the template is only exercised by release-plz when generating a changelog (Phase 3 dry-run will validate it).

### Task 2.3: Commit Phase 2

- [ ] **Step 1: Commit both files**

```bash
git -C /Users/rin/GitHub/zendriver-rs add release-plz.toml release-plz-changelog.toml
git -C /Users/rin/GitHub/zendriver-rs commit -m "$(cat <<'EOF'
chore(release): add release-plz config and changelog template

Configures release-plz for per-crate CHANGELOG generation, semver
checks, and conventional-commit-driven bumps. No workflows yet — those
land in Phase 3 of the migration.

Spec: docs/superpowers/specs/2026-05-25-publish-version-automation-design.md
EOF
)"
```

### Task 2.4: Add `RELEASE_PLZ_TOKEN` secret to the repo

**Files:**
- None (GitHub repo settings)

- [ ] **Step 1: Create a fine-grained PAT**

Open https://github.com/settings/personal-access-tokens/new and create a token with:
- **Resource owner:** `TurtIeSocks` (or whoever owns the repo)
- **Repository access:** only `TurtIeSocks/zendriver-rs`
- **Permissions:** `Contents: Read and write`, `Pull requests: Read and write`, `Workflows: Read and write` (required so release-plz can edit `.github/workflows/*` if a release ever touches them)
- **Expiration:** 1 year (set a calendar reminder to rotate)

Copy the token value.

- [ ] **Step 2: Add the secret to the repo**

```bash
gh secret set RELEASE_PLZ_TOKEN --repo TurtIeSocks/zendriver-rs --body "<paste-token>"
```

Expected: `✓ Set Actions secret RELEASE_PLZ_TOKEN for TurtIeSocks/zendriver-rs`.

- [ ] **Step 3: Confirm both required secrets exist**

```bash
gh secret list --repo TurtIeSocks/zendriver-rs | grep -E 'CARGO_REGISTRY_TOKEN|RELEASE_PLZ_TOKEN'
```

Expected: both lines present.

### Task 2.5: Open Phase-2 PR

- [ ] **Step 1: PR for config files**

```bash
git -C /Users/rin/GitHub/zendriver-rs push origin main:chore/release-plz-config
gh pr create --base main --head chore/release-plz-config \
  --title "chore(release): add release-plz config" \
  --body "$(cat <<'EOF'
## Summary
- Adds `release-plz.toml` and `release-plz-changelog.toml` at workspace root.
- Configures per-crate CHANGELOG generation, semver checks, and conventional-commit bumps.
- No workflows yet — Phase 3 of the migration adds those.

Spec: [`docs/superpowers/specs/2026-05-25-publish-version-automation-design.md`](docs/superpowers/specs/2026-05-25-publish-version-automation-design.md)

## Test plan
- [x] TOML syntactically valid
- [ ] CI green (no functional change expected)
EOF
)"
```

- [ ] **Step 2: Merge after CI green**

```bash
gh pr checks chore/release-plz-config
gh pr merge chore/release-plz-config --squash --delete-branch
git -C /Users/rin/GitHub/zendriver-rs checkout main && git -C /Users/rin/GitHub/zendriver-rs pull --ff-only
```

---

## Phase 3: Add release-plz workflows in dry-run mode

Land the two new workflows but **gate them on `workflow_dispatch` only** (no `on: push` yet) so they don't fire on every push and produce junk PRs.

### Task 3.1: Add `release-plz-pr.yml` in dry-run mode

**Files:**
- Create: `.github/workflows/release-plz-pr.yml`

- [ ] **Step 1: Write the workflow**

```yaml
name: Release PR (dry-run)

# Phase-3 staging: workflow_dispatch only. Phase 4 flips to `on: push`.

permissions:
  contents: write
  pull-requests: write

on:
  workflow_dispatch:

concurrency:
  group: release-plz-pr
  cancel-in-progress: false

jobs:
  release-plz-pr:
    name: Update release PR (dry-run)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          token: ${{ secrets.RELEASE_PLZ_TOKEN }}

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - uses: release-plz/action@v0.5.129
        with:
          command: release-pr
          # `dry-run` means: compute the bumps + changelog and log them,
          # but do not open or amend a PR. Safe to run repeatedly while
          # iterating on the config.
          # Once we're happy, Phase 4 removes this flag.
        env:
          GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

> **Note:** The `release-plz/action` does not have an action-level `--dry-run` flag — dry-run is the *absence* of write actions on the PR. To get a true preview without touching the PR state, we'll trigger this workflow once, inspect the action's logs (which show the computed bumps + diff), and only then proceed.

- [ ] **Step 2: No commit yet** — combined with the release workflow in Step below.

### Task 3.2: Add `release-plz-release.yml` in dry-run mode

**Files:**
- Create: `.github/workflows/release-plz-release.yml`

- [ ] **Step 1: Write the workflow**

```yaml
name: Release (dry-run)

# Phase-3 staging: workflow_dispatch only with --dry-run. Phase 4 flips
# to `on: push` and removes --dry-run.

permissions:
  contents: write

on:
  workflow_dispatch:
    inputs:
      really_publish:
        description: "If false (default), pass --dry-run to cargo publish. Leave false in Phase 3."
        type: boolean
        default: false

concurrency:
  group: release-plz-release
  cancel-in-progress: false

jobs:
  release-plz-release:
    name: Publish to crates.io (dry-run)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          token: ${{ secrets.RELEASE_PLZ_TOKEN }}

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - uses: release-plz/action@v0.5.129
        with:
          command: release
          # Phase 3: always force --dry-run regardless of input. This
          # makes `cargo publish` validate but skip the upload.
          # cargo_publish_args is passed straight to `cargo publish`.
          cargo_publish_args: ${{ inputs.really_publish && '' || '--dry-run' }}
        env:
          GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

- [ ] **Step 2: Verify YAML syntax**

```bash
yamllint /Users/rin/GitHub/zendriver-rs/.github/workflows/release-plz-pr.yml \
         /Users/rin/GitHub/zendriver-rs/.github/workflows/release-plz-release.yml \
  2>/dev/null || echo "(yamllint not installed — skip; GitHub will validate)"
```

Expected: no errors, or the skip-note.

### Task 3.3: Commit Phase 3 workflows

- [ ] **Step 1: Commit**

```bash
git -C /Users/rin/GitHub/zendriver-rs add .github/workflows/release-plz-pr.yml .github/workflows/release-plz-release.yml
git -C /Users/rin/GitHub/zendriver-rs commit -m "$(cat <<'EOF'
ci: add release-plz workflows in dry-run mode (Phase 3)

Adds release-plz-pr.yml and release-plz-release.yml gated on
workflow_dispatch only. The existing publish.yml is unchanged and still
the source of truth for actual publishes; this is staging for Phase 4.

Spec: docs/superpowers/specs/2026-05-25-publish-version-automation-design.md
EOF
)"
```

### Task 3.4: Open Phase-3 PR

- [ ] **Step 1: PR**

```bash
git -C /Users/rin/GitHub/zendriver-rs push origin main:ci/release-plz-workflows-staging
gh pr create --base main --head ci/release-plz-workflows-staging \
  --title "ci: add release-plz workflows in dry-run mode" \
  --body "$(cat <<'EOF'
## Summary
- Adds `release-plz-pr.yml` and `release-plz-release.yml`, both gated on `workflow_dispatch` only.
- Existing `publish.yml` is untouched — it's still authoritative until Phase 5.
- Allows us to invoke `release-pr` manually and inspect logs to validate the config before flipping to `on: push`.

## Test plan
- [ ] CI green on this PR
- [ ] After merge: manually trigger `Release PR (dry-run)` workflow, inspect logs for the computed bumps + changelog snippet (Task 3.5)
EOF
)"
```

- [ ] **Step 2: Merge after CI green**

```bash
gh pr checks ci/release-plz-workflows-staging
gh pr merge ci/release-plz-workflows-staging --squash --delete-branch
git -C /Users/rin/GitHub/zendriver-rs checkout main && git -C /Users/rin/GitHub/zendriver-rs pull --ff-only
```

### Task 3.5: First dry-run validation

**Files:**
- None (CI-only)

- [ ] **Step 1: Manually trigger `Release PR (dry-run)` workflow**

```bash
gh workflow run "Release PR (dry-run)" --repo TurtIeSocks/zendriver-rs
sleep 30
gh run list --workflow="Release PR (dry-run)" --limit 1
```

Expected: a run starts and completes.

- [ ] **Step 2: Inspect the run logs for computed bumps**

```bash
RUN_ID=$(gh run list --workflow="Release PR (dry-run)" --limit 1 --json databaseId -q '.[0].databaseId')
gh run view "$RUN_ID" --log | tee /tmp/release-plz-pr-dry-run.log
grep -E 'Bumping|new version|will be released' /tmp/release-plz-pr-dry-run.log
```

Expected to see (some subset, depending on commits since 0.1.0):
- `zendriver-interception`: bump from 0.1.0 to 0.2.0 (because of the `feat`-shaped `test-support` addition, or 0.1.1 if classified as fix). Inspect actual.
- `zendriver-mcp`: 0.1.0 → ???. May or may not bump depending on commits.
- Other crates with no source changes: no bump.

- [ ] **Step 3: Decide whether the proposed bumps are correct**

Compare against expectations from the spec:
- Crates with conventional-commit `feat:` since 0.1.0 → minor bump
- Crates with only `fix:` since 0.1.0 → patch bump
- Crates with no qualifying commits → no bump

If the proposed bumps look wrong, edit `release-plz-changelog.toml` (the `commit_parsers` table is the place to adjust classification) and re-run Task 3.5 from Step 1. Iterate until correct.

- [ ] **Step 4: Trigger `Release (dry-run)` workflow with `really_publish=false`**

```bash
gh workflow run "Release (dry-run)" --repo TurtIeSocks/zendriver-rs -f really_publish=false
sleep 60
RUN_ID=$(gh run list --workflow="Release (dry-run)" --limit 1 --json databaseId -q '.[0].databaseId')
gh run view "$RUN_ID" --log | tee /tmp/release-plz-release-dry-run.log
grep -E 'Packaging|Uploading|aborting upload due to dry run' /tmp/release-plz-release-dry-run.log
```

Expected: per-crate `Packaging` + `aborting upload due to dry run` lines for the crates the release-pr step would bump.

- [ ] **Step 5: No commit** — this task is validation. Any config tweaks from Step 3 go into a separate small commit + PR (`chore(release-plz): tune commit parsers`).

---

## Phase 4: Wire up `on: push` triggers

Flip both workflows from `workflow_dispatch` only to `on: push: branches: [main]`. **`publish.yml` is still active** at this point — both old and new pipelines will run on each push, but the new release-plz pipeline only opens a PR (no double-publish risk).

### Task 4.1: Update `release-plz-pr.yml` triggers

**Files:**
- Modify: `.github/workflows/release-plz-pr.yml`

- [ ] **Step 1: Replace the workflow file**

```yaml
name: Release PR

permissions:
  contents: write
  pull-requests: write

on:
  push:
    branches: [main]
    # Don't fire on the release PR's own merge commit (release-plz
    # commits are titled "chore: release ..."); the release workflow
    # picks that up instead.
  workflow_dispatch:

concurrency:
  group: release-plz-pr
  cancel-in-progress: false

jobs:
  release-plz-pr:
    name: Update release PR
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          token: ${{ secrets.RELEASE_PLZ_TOKEN }}

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - uses: release-plz/action@v0.5.129
        with:
          command: release-pr
        env:
          GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

(Name is no longer "(dry-run)". `workflow_dispatch` is preserved for manual re-runs.)

### Task 4.2: Update `release-plz-release.yml` triggers

**Files:**
- Modify: `.github/workflows/release-plz-release.yml`

- [ ] **Step 1: Replace the workflow file**

```yaml
name: Release

permissions:
  contents: write

on:
  push:
    branches: [main]
  workflow_dispatch:

concurrency:
  group: release-plz-release
  cancel-in-progress: false

jobs:
  release-plz-release:
    name: Publish to crates.io
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          token: ${{ secrets.RELEASE_PLZ_TOKEN }}

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - uses: release-plz/action@v0.5.129
        with:
          command: release
        env:
          GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

(Name is no longer "(dry-run)". `cargo_publish_args` removed — full publish on real release merge. `release-plz release` no-ops if the latest commit isn't a release commit, so the `on: push` trigger is safe.)

### Task 4.3: Commit Phase 4

- [ ] **Step 1: Commit**

```bash
git -C /Users/rin/GitHub/zendriver-rs add .github/workflows/release-plz-pr.yml .github/workflows/release-plz-release.yml
git -C /Users/rin/GitHub/zendriver-rs commit -m "$(cat <<'EOF'
ci: wire release-plz workflows to on:push trigger (Phase 4)

- release-plz-pr.yml now opens/amends release PRs on every main push.
- release-plz-release.yml publishes on release-PR merge.
- Old publish.yml still active; both pipelines coexist this phase.

Spec: docs/superpowers/specs/2026-05-25-publish-version-automation-design.md
EOF
)"
```

### Task 4.4: Open Phase-4 PR

- [ ] **Step 1: PR**

```bash
git -C /Users/rin/GitHub/zendriver-rs push origin main:ci/release-plz-wire-push-trigger
gh pr create --base main --head ci/release-plz-wire-push-trigger \
  --title "ci: wire release-plz workflows to on:push trigger" \
  --body "$(cat <<'EOF'
## Summary
- Flips `release-plz-pr.yml` and `release-plz-release.yml` from `workflow_dispatch` only to `on: push: branches: [main]`.
- Existing `publish.yml` still runs in parallel — release-plz only opens a PR, so no double-publish risk yet.
- This is the last phase before cutover. Phase 5 will delete `publish.yml` after the first release-plz PR successfully publishes.

## Test plan
- [ ] CI green
- [ ] After merge: release-plz opens a "chore: release" PR with the accumulated bumps from the commits between 0.1.0 and now
- [ ] Verify the PR's diff matches the dry-run output from Task 3.5
EOF
)"
```

- [ ] **Step 2: Merge after CI green**

```bash
gh pr checks ci/release-plz-wire-push-trigger
gh pr merge ci/release-plz-wire-push-trigger --squash --delete-branch
git -C /Users/rin/GitHub/zendriver-rs checkout main && git -C /Users/rin/GitHub/zendriver-rs pull --ff-only
```

### Task 4.5: Validate release-plz opens its first real PR

- [ ] **Step 1: Wait for the workflow to run**

```bash
sleep 90
gh run list --workflow="Release PR" --limit 1
```

Expected: most recent run shows `success`.

- [ ] **Step 2: Confirm a release PR was opened**

```bash
gh pr list --author "github-actions[bot]" --state open --search 'chore: release'
```

Expected: one PR titled something like `chore: release` with the per-crate bumps in its description.

- [ ] **Step 3: Inspect the release PR's diff**

```bash
PR_NUMBER=$(gh pr list --author "github-actions[bot]" --state open --search 'chore: release' --json number -q '.[0].number')
gh pr diff "$PR_NUMBER"
```

Expected: per-crate `version = "0.X.Y"` bumps + per-crate `CHANGELOG.md` additions matching what Task 3.5 predicted.

- [ ] **Step 4: Do NOT merge the release PR yet** — Phase 5 does that.

---

## Phase 5: Cutover — delete `publish.yml`, merge first release PR

The release PR from Task 4.5 is the proof that release-plz is doing the right thing. Now we remove the old publisher and merge the release PR to perform the first end-to-end automated release.

### Task 5.1: Delete `publish.yml`

**Files:**
- Delete: `.github/workflows/publish.yml`

- [ ] **Step 1: Delete the file**

```bash
git -C /Users/rin/GitHub/zendriver-rs rm .github/workflows/publish.yml
```

- [ ] **Step 2: Commit**

```bash
git -C /Users/rin/GitHub/zendriver-rs commit -m "$(cat <<'EOF'
ci: delete publish.yml — superseded by release-plz workflows

The manual version-gated publish.yml is replaced by release-plz's
two-workflow pattern (release-plz-pr.yml opens the release PR,
release-plz-release.yml publishes on merge).

Spec: docs/superpowers/specs/2026-05-25-publish-version-automation-design.md
Migration plan: docs/superpowers/plans/2026-05-25-publish-version-automation.md
EOF
)"
```

### Task 5.2: PR + merge the deletion

- [ ] **Step 1: PR**

```bash
git -C /Users/rin/GitHub/zendriver-rs push origin main:ci/delete-publish-yml
gh pr create --base main --head ci/delete-publish-yml \
  --title "ci: delete publish.yml — superseded by release-plz" \
  --body "$(cat <<'EOF'
## Summary
- Removes `.github/workflows/publish.yml`.
- release-plz workflows (added in Phase 3/4) are now the only path to crates.io.

## Test plan
- [ ] CI green
- [ ] After merge: open release PR (#???) is still present and ready to merge in Task 5.3
EOF
)"
gh pr checks ci/delete-publish-yml
gh pr merge ci/delete-publish-yml --squash --delete-branch
git -C /Users/rin/GitHub/zendriver-rs checkout main && git -C /Users/rin/GitHub/zendriver-rs pull --ff-only
```

**Note:** This push to main will trigger `release-plz-pr.yml` again. release-plz will re-evaluate and update the open release PR with this latest commit. That's fine.

### Task 5.3: Merge the first release PR

**Files:**
- None (the PR's own commit is the change)

- [ ] **Step 1: Confirm the release PR is still open and up-to-date**

```bash
PR_NUMBER=$(gh pr list --author "github-actions[bot]" --state open --search 'chore: release' --json number -q '.[0].number')
gh pr view "$PR_NUMBER"
gh pr checks "$PR_NUMBER"
```

Expected: PR is open, CI green.

- [ ] **Step 2: Review the PR's diff one more time**

```bash
gh pr diff "$PR_NUMBER"
```

Expected: per-crate version bumps + per-crate `CHANGELOG.md` updates only. No surprise file changes.

- [ ] **Step 3: Merge the release PR**

```bash
gh pr merge "$PR_NUMBER" --squash
```

Expected: merge succeeds. This commit becomes the "release commit" that `release-plz-release.yml` will detect.

- [ ] **Step 4: Watch the Release workflow execute**

```bash
sleep 60
gh run watch "$(gh run list --workflow=Release --limit 1 --json databaseId -q '.[0].databaseId')"
```

Expected: workflow streams the per-crate publish steps. Each `cargo publish` succeeds; release-plz creates git tags and GitHub Releases.

- [ ] **Step 5: Verify crates.io shows new versions**

```bash
for crate in zendriver-transport zendriver-stealth zendriver-interception zendriver-fetcher zendriver-cloudflare zendriver zendriver-mcp; do
  echo -n "$crate: "
  curl -s "https://crates.io/api/v1/crates/$crate" | jq -r '.crate.max_version'
done
```

Expected: each crate's `max_version` matches the bump from the release PR.

- [ ] **Step 6: Verify GitHub Releases were created**

```bash
gh release list --limit 10
```

Expected: one release per bumped crate (release-plz tags as e.g. `zendriver-interception-v0.2.0`).

### Task 5.4: Update top-level CHANGELOG.md with cutover entry

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add a "## [Unreleased]" section noting the release-automation cutover (the workspace-level CHANGELOG.md is human-curated; release-plz does not touch it)**

Edit `CHANGELOG.md` and insert near the top, after the header lines:

```markdown
## [Unreleased]

### Changed
- Per-crate version automation via release-plz (see `docs/superpowers/specs/2026-05-25-publish-version-automation-design.md`).
  Each crate now versions independently based on conventional commits.
  Per-crate `CHANGELOG.md` files (next to each crate's `Cargo.toml`)
  are the authoritative changelog source going forward; this top-level
  file remains for human-curated release narrative.
```

- [ ] **Step 2: Commit**

```bash
git -C /Users/rin/GitHub/zendriver-rs add CHANGELOG.md
git -C /Users/rin/GitHub/zendriver-rs commit -m "$(cat <<'EOF'
docs(changelog): note release-plz cutover

Adds an [Unreleased] section explaining the move to per-crate
automated changelogs.
EOF
)"
```

- [ ] **Step 3: PR + merge (small enough to skip the formal PR if your repo allows direct main pushes for docs)**

```bash
git -C /Users/rin/GitHub/zendriver-rs push origin main:docs/changelog-release-plz-note
gh pr create --base main --head docs/changelog-release-plz-note \
  --title "docs(changelog): note release-plz cutover" \
  --body "_Docs-only note about the publish automation migration._"
gh pr checks docs/changelog-release-plz-note
gh pr merge docs/changelog-release-plz-note --squash --delete-branch
git -C /Users/rin/GitHub/zendriver-rs checkout main && git -C /Users/rin/GitHub/zendriver-rs pull --ff-only
```

---

## Post-migration verification

### Task POST.1: Smoke-test the full cycle with a small contrived change

**Files:**
- Modify: `crates/zendriver-transport/src/lib.rs` (or any low-risk file)

- [ ] **Step 1: Push a `fix:` commit to verify release-plz reacts**

```bash
git -C /Users/rin/GitHub/zendriver-rs checkout -b smoke/release-plz-cycle
# Make a trivial whitespace-fix commit to a transport file:
echo "" >> /Users/rin/GitHub/zendriver-rs/crates/zendriver-transport/src/lib.rs
git -C /Users/rin/GitHub/zendriver-rs add crates/zendriver-transport/src/lib.rs
git -C /Users/rin/GitHub/zendriver-rs commit -m "fix(zendriver-transport): whitespace nudge to smoke-test release-plz"
git -C /Users/rin/GitHub/zendriver-rs push origin smoke/release-plz-cycle
gh pr create --base main --head smoke/release-plz-cycle \
  --title "fix(zendriver-transport): whitespace nudge to smoke-test release-plz" \
  --body "Verifies the release-plz pipeline triggers a patch bump on a fix: commit."
gh pr checks smoke/release-plz-cycle
gh pr merge smoke/release-plz-cycle --squash --delete-branch
git -C /Users/rin/GitHub/zendriver-rs checkout main && git -C /Users/rin/GitHub/zendriver-rs pull --ff-only
```

- [ ] **Step 2: Wait for release-plz to update the release PR**

```bash
sleep 90
gh pr list --author "github-actions[bot]" --state open --search 'chore: release'
```

Expected: a release PR exists, and its diff shows `zendriver-transport` bumped by one patch (e.g. `0.1.1` → `0.1.2`, or whatever the previous post-cutover version was + 1 patch).

- [ ] **Step 3: Confirm cascading dep updates**

```bash
PR_NUMBER=$(gh pr list --author "github-actions[bot]" --state open --search 'chore: release' --json number -q '.[0].number')
gh pr diff "$PR_NUMBER" -- Cargo.toml
```

Expected: `[workspace.dependencies]`'s `zendriver-transport` entry shows the new version pin.

- [ ] **Step 4: Decide whether to merge or close the smoke PR**

If the bump looks correct, merge it (this becomes a real release). If you'd rather not publish a whitespace-fix release, close the release PR without merging — release-plz will re-open it next time a real change lands.

---

## Self-Review (run after writing the plan)

### Spec coverage

| Spec requirement | Plan task |
|------------------|-----------|
| Per-crate change detection → bump | Phase 3 (validation) + Phase 4 (live) |
| Conventional-commit bump magnitude | release-plz-changelog.toml commit_parsers (Task 2.2) |
| Review checkpoint (release PR) | Whole architecture — no auto-publish |
| Per-crate CHANGELOG.md | release-plz.toml `changelog_update = true` (Task 2.1) |
| Cascade internal dep bumps | release-plz handles automatically via `[workspace.dependencies]` (Task 1.2 verifies pins exist) |
| Security: no untrusted input in shell | New workflows have no `run:` blocks that interpolate event input |
| `cargo-semver-checks` enforced | release-plz.toml `semver_check = true` (Task 2.1) |
| `release_always = false` initially | release-plz.toml default (Task 2.1 — commented) |
| PAT for downstream-workflow triggering | RELEASE_PLZ_TOKEN (Task 2.4) |
| `zendriver-mcp` `publish = true` | release-plz.toml `[[package]] name = "zendriver-mcp"` (Task 2.1) |
| Replace publish.yml | Phase 5 (Task 5.1) |
| Path-only dev-dep fix stays | Phase 0 lands it; no later task reverts |
| 5-phase migration | Phases 1–5 + Phase 0 prerequisite |

All spec requirements covered.

### Placeholder scan

No `TBD`, `TODO`, "implement later". Every step has either a concrete command, code block, or explicit "no action" with reason. ✓

### Type consistency

- Workflow names: `Release PR` and `Release` consistently used in Phase 4–5 (after rename from dry-run variants).
- Workflow file names: `release-plz-pr.yml` and `release-plz-release.yml` consistent throughout.
- Secret names: `RELEASE_PLZ_TOKEN`, `CARGO_REGISTRY_TOKEN` consistent.
- Action version: `release-plz/action@v0.5.129` consistent across both workflows.

All consistent.

### Scope check

Single focused migration. Five phases + Phase 0 prereq + Post smoke test. Self-contained. Fits in one plan.

---

## Notes for the executing agent

1. **Pre-1.0 versioning:** Per `~/.claude/projects/-Users-rin-GitHub-zendriver-rs/memory/api-churn-acceptable-pre-release.md`, breaking changes pre-1.0 get a minor bump (0.1.0 → 0.2.0), not major. release-plz's default behavior matches this — no special config needed.

2. **Topological ordering:** release-plz computes the publish DAG from `[dependencies]` declarations automatically; you do not need to hard-code the order (unlike the old `publish.yml`).

3. **30-second index-propagation sleep:** the old `publish.yml` had a 30-second sleep between crates to wait for crates.io index propagation. release-plz handles this internally — no manual sleep needed in the new workflows.

4. **If `RELEASE_PLZ_TOKEN` is missing on first Phase-3 run:** the workflow will fail with a permissions error. Confirm `gh secret list` shows it before triggering Task 3.5.

5. **If release-plz proposes a version that surprises you:** check `release-plz-changelog.toml`'s `[git].commit_parsers` — that's the source of bump-magnitude classification. Tweak there, not in release-plz.toml.
