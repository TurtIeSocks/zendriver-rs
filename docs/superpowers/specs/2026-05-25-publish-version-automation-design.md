# Publish Version Automation — Design

**Date:** 2026-05-25
**Status:** Draft
**Author:** rin (delegated to Claude)
**Related:** [Phase 6 release design](2026-05-23-zendriver-rs-phase6-release-design.md), [zendriver-mcp design](2026-05-24-zendriver-rs-mcp-server-design.md)

## Problem

The current `Publish` workflow (`.github/workflows/publish.yml`) compares each crate's local `Cargo.toml` version against `crates.io`'s `max_version` and publishes only when they differ. This is correct gating, but version bumps are **manual**: a contributor has to edit `Cargo.toml` (or workspace `version`) in the PR.

Two failures resulted in the recent `zendriver-mcp` merge:

1. **Forgotten bump.** PR changed `crates/zendriver-interception/src/actor.rs` (added `InterceptHandle::for_tests`) and `crates/zendriver-interception/Cargo.toml` (added `test-support` feature). Version stayed at `0.1.0`. Workflow saw `local_ver == remote_ver`, skipped publish. The new feature/code never reached crates.io.
2. **Cross-crate feature mismatch.** `zendriver-mcp` dev-dep on `zendriver-interception` requested the new `test-support` feature, but the cargo-publish verification step resolved that dep against the **published** 0.1.0 on crates.io (which lacks the feature), not the local workspace path. → publish failed with `failed to select a version for zendriver-interception`.

Failure (2) is being patched separately (path-only dev-dep in `zendriver-mcp/Cargo.toml`, already committed as `49ececc`). This spec addresses failure (1): **how do we ensure source changes to a crate translate to a published version bump without contributors having to remember?**

## Goals

- Per-crate change detection: when files under `crates/<X>/` change between releases, `<X>`'s version bumps.
- Bump magnitude inferred from commit history (conventional commits) — not manually chosen.
- A **review checkpoint** before any publish — bumps land in a PR, not a force-push.
- Automatic per-crate `CHANGELOG.md` so consumers can see what changed.
- Internal dep version requirements update when a depended-on crate bumps.
- Keep the security posture of the current workflow (no untrusted GitHub event input interpolated into shell).

## Non-goals

- Replacing the human in the release loop. We want a PR, not a fully-autonomous publisher.
- Per-feature-flag changelog detail. Conventional-commit summaries are enough.
- Replacing the top-level human-curated `CHANGELOG.md`. That stays for narrative release notes.

---

## Approach: release-plz

[release-plz](https://release-plz.dev) is the dominant Rust workspace release automation. It implements the exact workflow we want:

1. **`release-plz-pr` workflow** runs on push to `main`. Detects which crates' source changed since their last published version (by diffing against the `.crate` file on crates.io). For each changed crate:
   - Bumps version per conventional commits (`fix:` → patch, `feat:` → minor, `feat!:`/`BREAKING CHANGE:` → minor in pre-1.0, major in post-1.0).
   - Updates `[workspace.dependencies]` so internal deps move with their target.
   - Writes/updates per-crate `CHANGELOG.md` from commit messages.
   - Opens (or amends) a single "chore: release" PR with all of the above.
2. **`release-plz-release` workflow** runs on push to `main`. If the latest commit on `main` is a release commit (i.e. the release PR was just merged), it publishes the affected crates to crates.io in topological order. If not, it no-ops.

This replaces the current `publish.yml` entirely. The review checkpoint is built in (the release PR), changelogs are automatic, dep cascade is handled by the tool.

### Why release-plz over a hand-rolled solution

A hand-rolled equivalent would need:
- Per-crate change detection vs each crate's last published commit (requires either git tags per crate or fetching `.crate` files from crates.io to diff).
- Conventional-commit parsing.
- Internal dep cascade logic (if `zendriver-interception` bumps to `0.2.0`, every workspace dep entry pointing at it must bump too).
- Auto-commit machinery that doesn't recursively trigger itself (`[skip ci]`, path-filter tricks).
- Race-condition handling for concurrent release PRs.

This is several hundred lines of bash for a solved problem. release-plz is mature, has ~3.5k GitHub stars, and is what `cargo`, `serde`, `axum`, and most large workspaces use.

### Why release-plz over `cargo-smart-release` / `cargo-release`

- `cargo-release`: requires manual invocation — doesn't solve the "contributor forgot" problem.
- `cargo-smart-release`: lower activity, no auto-PR workflow, less GitHub Actions integration.

---

## Workspace structure changes

### Per-crate versions (breaks current shared `workspace.version`)

Current state: `Cargo.toml` has `workspace.package.version = "0.1.0"`, every crate inherits via `version.workspace = true`. Any change to any crate forces a "bump all" decision.

New state: each crate gets its own `version` field. `workspace.package.version` is removed.

```toml
# crates/zendriver-interception/Cargo.toml
[package]
name = "zendriver-interception"
version = "0.1.0"   # ← explicit, no longer .workspace
edition.workspace = true
# ...
```

Workspace `Cargo.toml` keeps version pins in `[workspace.dependencies]`, which release-plz updates per bump:

```toml
[workspace.dependencies]
zendriver-interception = { path = "crates/zendriver-interception", version = "0.1.0" }
# ↓ after release-plz bumps interception to 0.2.0:
zendriver-interception = { path = "crates/zendriver-interception", version = "0.2.0" }
```

All other `workspace.package.*` fields (edition, rust-version, license, repository, authors) stay shared. Only `version` is split.

### `release-plz.toml` at workspace root

```toml
[workspace]
# Run cargo-semver-checks before bumping, so API breaks force a minor bump pre-1.0.
semver_check = true

# Generate per-crate CHANGELOG.md.
changelog_update = true

# Create a GitHub Release for each published version.
git_release_enable = true

# Use conventional commits for bump magnitude inference.
changelog_config = "release-plz-changelog.toml"

# Don't release if there are no commits between releases.
publish_no_verify = false

[[package]]
name = "zendriver-mcp"
# Binary crate — still released for `cargo install zendriver-mcp` users.
publish = true
```

Plus a `release-plz-changelog.toml` for the changelog template (uses git-cliff under the hood).

### Pre-1.0 semver convention

Per the project's pre-release stance (see `~/.claude/projects/-Users-rin-GitHub-zendriver-rs/memory/api-churn-acceptable-pre-release.md` — no public users until Phase 6), we use **standard pre-1.0 semver**:

- `fix:` → patch (`0.1.0` → `0.1.1`)
- `feat:` → minor (`0.1.0` → `0.2.0`)
- `feat!:` / `BREAKING CHANGE:` → minor (`0.1.0` → `0.2.0`) — pre-1.0, breaking is allowed without major
- `chore:`, `docs:`, `ci:`, `tests:`, `refactor:` → no bump (release-plz's default `[changelog].commit_parsers` skips these unless they touch the crate's source files, in which case they fall through to patch)

`cargo-semver-checks` enforces that an actual API break isn't released as a patch — it forces minor.

---

## New workflows

Both replace `.github/workflows/publish.yml`.

### `.github/workflows/release-plz-pr.yml`

```yaml
name: Release PR

# Open or update the "chore: release" PR whenever changes land on main.
permissions:
  contents: write
  pull-requests: write

on:
  push:
    branches: [main]

# Don't race with concurrent main pushes.
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
          fetch-depth: 0   # release-plz needs full history for changelog
          token: ${{ secrets.RELEASE_PLZ_TOKEN }}
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: release-plz/action@v0.5
        with:
          command: release-pr
        env:
          GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

### `.github/workflows/release-plz-release.yml`

```yaml
name: Release

# Publish to crates.io when the release PR is merged into main.
permissions:
  contents: write   # for git tag + GitHub Release

on:
  push:
    branches: [main]

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
      - uses: release-plz/action@v0.5
        with:
          command: release
        env:
          GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

### Required secrets

- `CARGO_REGISTRY_TOKEN` — already set.
- `RELEASE_PLZ_TOKEN` — **new**. A GitHub PAT (or fine-grained PAT) with `contents: write` + `pull-requests: write` on this repo. Needed because the default `GITHUB_TOKEN` cannot trigger downstream workflows (the release PR being merged needs to fire `release-plz-release.yml`, and the default token's events don't trigger `on: push`). [Documented behavior.](https://docs.github.com/en/actions/security-guides/automatic-token-authentication#using-the-github_token-in-a-workflow)

  Alternative: use a GitHub App. PAT is simpler for a solo-maintained repo. Set as a repo secret.

### Removed

- `.github/workflows/publish.yml` deleted. Its responsibilities are subsumed by the two release-plz workflows.

---

## Data & file flow

```
PR merged to main
       │
       ▼
release-plz-pr.yml fires
       │
       ▼
release-plz computes per-crate diffs vs crates.io latest
   │
   ├── For each changed crate: bump version, update dep pins, append CHANGELOG entry
   │
   └── Open or amend "chore: release" PR
              │
              ▼
       Human reviews PR (rejects/edits/approves)
              │
              ▼
       PR merged to main
              │
              ▼
release-plz-release.yml fires
       │
       ▼
   Detects latest commit is a release commit → publishes in topo order
       │
       └── Creates git tags + GitHub Releases per crate
```

The release PR is **single, long-lived, and amended** — each new commit to main updates it rather than opening a new one. Merging it triggers a publish; closing it without merging discards the bumps.

---

## Error handling

### Failed publish on one crate mid-topo-order
release-plz publishes in topological order. If a mid-list crate fails (e.g. transient crates.io error), it stops and reports. Already-published crates stay published; un-published ones can be re-attempted by re-running the workflow. release-plz idempotently skips already-published versions.

### Conventional-commit non-compliance
A commit message like `Updated stuff` with no prefix gets categorized as "uncategorized" by git-cliff and triggers no bump on its own. If it's the only commit affecting a crate, that crate **doesn't get a release** even though its files changed.

Mitigation: optionally enable `release_always = true` per-crate, which forces a patch bump on any file change regardless of commit message. We opt-in to this for the highest-traffic crates (`zendriver`, `zendriver-mcp`) and accept the risk for low-traffic ones.

**Decision:** Start with `release_always = false` (default). Revisit if we lose releases. Add a `verify-conv-commits.yml` CI check on PRs to reject non-conventional titles — gives contributors fast feedback.

### Breaking API change not caught by `cargo-semver-checks`
Possible — `cargo-semver-checks` has known false negatives. We accept the risk pre-1.0. Post-1.0 we can add manual review on the release PR.

### Race: two PRs merge while release PR is open
release-plz-pr amends the existing release PR on each main push. If a normal PR merges between two `release-plz-pr` runs, the next run picks it up and adds its bump to the PR. No race.

### Release-plz GitHub Action down / broken
The current `publish.yml` is preserved in git history. Roll back by reverting the workflow swap commit.

---

## Testing strategy

Before swapping workflows:

1. **Dry-run on a fresh branch.** Cut a `release-plz-test` branch. Add the new workflows (with `release-plz release-pr --dry-run` and `release-plz release --dry-run` flags). Push a commit with `feat(zendriver-interception): test bump`. Confirm the action opens a "release PR" with the expected version bumps and CHANGELOG.
2. **Verify cargo-semver-checks fires.** Add a commit with a deliberately breaking change (rename a public function). Confirm release-plz infers a minor bump.
3. **Verify dep cascade.** Bump `zendriver-interception`. Confirm `zendriver`'s and `zendriver-mcp`'s `workspace.dependencies` entries get updated in the same release PR.
4. **Verify topo publish.** Run `release-plz release --dry-run` against a state with multiple crates pending publish. Confirm order matches the dependency DAG.

Only after all four pass: delete `publish.yml`, land the new workflows.

---

## Migration plan (summary, full plan in writing-plans)

**Phase 0 — Land the path-only dev-dep fix** (already committed locally as `49ececc`, needs push). This unblocks today's publish independent of the release-plz work.

**Phase 1 — Per-crate versions.** Split `workspace.package.version` into per-crate `version = "0.1.0"`. Update `[workspace.dependencies]` entries to keep `version = "0.1.0"` pins. No behavior change yet — all crates still at 0.1.0.

**Phase 2 — Add release-plz config.** Write `release-plz.toml` + `release-plz-changelog.toml`. Add `RELEASE_PLZ_TOKEN` secret to the repo.

**Phase 3 — Add new workflows in `--dry-run` mode.** Land `release-plz-pr.yml` and `release-plz-release.yml` with dry-run flags. Trigger on workflow_dispatch first (not `on: push`) so we can iterate without producing junk PRs.

**Phase 4 — Wire up `on: push`.** Switch triggers to `on: push: branches: [main]`. Push a test commit. Confirm a release PR opens.

**Phase 5 — Cut over.** Delete `publish.yml`. Merge the first real release PR. Confirm publish-to-crates.io works end-to-end. This is the first "real" release under the new system and will include all the source changes accumulated since `0.1.0` (which is currently the state of `zendriver-interception`, `zendriver-mcp`, and any others).

---

## Assumptions (decisions made on your behalf)

These are the judgement calls. **Push back on any that don't match your intent before I write the implementation plan.**

| # | Assumption | Alternative if rejected |
|---|------------|--------------------------|
| 1 | **Tool: release-plz** | Hand-rolled bash + cargo-edit, or cargo-smart-release |
| 2 | **Per-crate versions** (break `workspace.package.version`) | Keep shared workspace version; bump all crates together on any change |
| 3 | **Pre-1.0 semver: breaking → minor**, not major | Use major bumps even pre-1.0 |
| 4 | **Conventional commits drive bump magnitude** | Always-patch, or PR-label-driven, or manual `release-plz set-version` |
| 5 | **Release PR pattern**, not auto-publish on every main push | Auto-publish (no review checkpoint) |
| 6 | **Replace publish.yml entirely**, don't run both | Run both in parallel during transition |
| 7 | **Per-crate CHANGELOG.md** + keep top-level `CHANGELOG.md` for human curation | Only top-level, or only per-crate |
| 8 | **Enable `cargo-semver-checks`** to catch API breaks | Off (faster CI, but trusts contributors more) |
| 9 | **`release_always = false`** initially — rely on conventional commits to trigger releases | `release_always = true` — every source change → patch bump |
| 10 | **PAT (`RELEASE_PLZ_TOKEN`)** for triggering downstream workflows | GitHub App (more setup, better security) |
| 11 | **Trigger on push to main + workflow_dispatch** (same as current `publish.yml`) | Schedule-based (daily release PR) |
| 12 | **`zendriver-mcp` is `publish = true`** (it's a binary, but `cargo install` users want it) | `publish = false`, distribute via release binaries only |
| 13 | **Path-only dev-dep fix from earlier stays** — it's still good practice for internal-only test features even with release-plz | Revert and let release-plz republish `zendriver-interception` with `test-support` exposed |
| 14 | **No conventional-commit lint** on PRs at first; revisit if we lose releases | Add a `verify-conv-commits.yml` PR gate from day one |
| 15 | **Migration runs in 5 phases**, dry-run validated before cutover | Big-bang cutover; or 1-2 phase fast switch |

## Open questions for you

(None — every meaningful call has been made above. If any assumption above is wrong, flag it.)

## References

- [release-plz docs](https://release-plz.ieni.dev/)
- [release-plz action](https://github.com/release-plz/action)
- [cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks)
- [Conventional Commits](https://www.conventionalcommits.org/)
- [Current `publish.yml`](../../.github/workflows/publish.yml) (to be removed)
- [Phase 6 release design](2026-05-23-zendriver-rs-phase6-release-design.md)
