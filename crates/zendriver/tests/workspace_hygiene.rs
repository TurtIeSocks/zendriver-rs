//! Guards against re-introducing the shared-README changelog-misattribution bug.
//!
//! Seven sibling crates used to point their Cargo `readme` field at the shared
//! workspace-root `README.md`. release-plz scopes each crate's changelog to
//! commits whose changed files intersect that crate's *published file set*
//! (`cargo package --list`, which includes the `readme` target) — so every commit
//! touching the root README looked like a change to all seven crates, polluting
//! their `CHANGELOG.md`s with entries for features they never received. Each crate
//! now has its own short README; these tests fail if the shared pointer returns.

// Setup failures in a hygiene test legitimately panic to fail the test loudly.
#![allow(clippy::panic)]

use std::path::Path;

const SIBLING_CRATES: &[&str] = &[
    "zendriver-transport",
    "zendriver-stealth",
    "zendriver-interception",
    "zendriver-fetcher",
    "zendriver-imperva",
    "zendriver-cloudflare",
    "zendriver-datadome",
];

#[test]
fn sibling_crates_have_their_own_readme_not_the_shared_root_one() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for crate_name in SIBLING_CRATES {
        let manifest_path = workspace_root
            .join("crates")
            .join(crate_name)
            .join("Cargo.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("failed to read {manifest_path:?}: {e}"));
        assert!(
            !manifest.contains(r#"readme = "../../README.md""#),
            "{crate_name} points `readme` at the shared workspace README again — \
             this makes release-plz misattribute changelog entries to it. \
             Give it its own README.md instead."
        );
        let own_readme = workspace_root
            .join("crates")
            .join(crate_name)
            .join("README.md");
        assert!(
            own_readme.exists(),
            "{crate_name} is missing its own README.md"
        );
    }
}

#[test]
fn dependency_only_bumps_get_a_labeled_changelog_entry_not_silence() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = std::fs::read_to_string(workspace_root.join("release-plz-changelog.toml"))
        .expect("release-plz-changelog.toml must exist at the workspace root");
    // Quote-agnostic (the regex is a TOML literal string, single-quoted): match
    // the regex content itself, not the `message = "…"` wrapper.
    let specific_rule = r#"^chore: update Cargo\.(toml|lock) dependencies$"#;
    assert!(
        config.contains(specific_rule),
        "missing the labeled commit_parser rule for release-plz's synthetic \
         dependency-only-bump commit messages"
    );
    let specific_pos = config.find(specific_rule).unwrap();
    let generic_skip_pos = config.find(r#"message = "^chore""#).unwrap();
    assert!(
        specific_pos < generic_skip_pos,
        "the labeled dependency-bump rule must come BEFORE the generic ^chore skip \
         rule — commit_parsers is first-match-wins"
    );
}
