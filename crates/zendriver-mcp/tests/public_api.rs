//! Enforces that every NEW public `zendriver` API item (present in the current
//! output of `cargo public-api -p zendriver --all-features` but absent from the
//! checked-in baseline) has an entry in `mcp-coverage-ledger.toml`.
//!
//! This test ONLY runs in the dedicated nightly CI job
//! (`cargo test -p zendriver-mcp --features public-api-check --test public_api`).
//! It must NOT compile or run as part of the default stable test suite.
//!
//! Pinned tool: `cargo-public-api v0.52.0`
//! Install: `cargo install cargo-public-api --locked --version 0.52.0`
//!
//! # Bootstrap behaviour
//!
//! If `public-api-baseline.txt` is absent or empty the test GENERATES the
//! baseline (writes the current public API) and PASSES with a printed note.
//! This lets the first nightly CI run seed the baseline without pre-generating
//! it locally. Every subsequent run diffs against the seeded file.
//!
//! # Updating the baseline
//!
//! When you ADD a new public API item that has a ledger entry, regenerate
//! the baseline with:
//!
//!   cargo +nightly public-api -p zendriver --all-features \
//!       > crates/zendriver-mcp/public-api-baseline.txt
//!
//! Then commit both the updated baseline and the new ledger entry together.

#![cfg(feature = "public-api-check")]
// Test code legitimately uses panic! for setup failures and char pattern
// matching via closure — suppress lints that don't apply to test helpers.
#![allow(clippy::panic, clippy::manual_pattern_char_comparison)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR resolves to crates/zendriver-mcp at test time.
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set; run via `cargo test`");
    PathBuf::from(manifest)
        .parent()
        .expect("crates/")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn baseline_path(root: &Path) -> PathBuf {
    root.join("crates/zendriver-mcp/public-api-baseline.txt")
}

fn ledger_path(root: &Path) -> PathBuf {
    root.join("crates/zendriver-mcp/mcp-coverage-ledger.toml")
}

/// Run `cargo +<toolchain> public-api -p zendriver --all-features` and return
/// its stdout line-by-line (excluding diagnostics lines on stderr).
///
/// The toolchain is read from `PUBLIC_API_TOOLCHAIN` (default `nightly`). CI
/// PINS this to a specific dated nightly so rustdoc-JSON drift can't
/// spuriously surface "new" items on every nightly release; regenerate the
/// baseline with the SAME pinned toolchain when it changes.
fn current_public_api(root: &Path) -> Vec<String> {
    let toolchain = std::env::var("PUBLIC_API_TOOLCHAIN").unwrap_or_else(|_| "nightly".to_string());
    let output = Command::new("cargo")
        .args([
            &format!("+{toolchain}"),
            "public-api",
            "-p",
            "zendriver",
            "--all-features",
        ])
        .current_dir(root)
        .output()
        .expect("cargo +<toolchain> public-api failed to launch; ensure cargo-public-api v0.52.0 is installed");

    assert!(
        output.status.success(),
        "cargo +nightly public-api failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .expect("cargo-public-api output is not valid UTF-8")
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Extract the canonical path from a public-api line.
///
/// `cargo-public-api` lines look like:
///   `pub fn zendriver::tab::Tab::request(&self) -> …`
///
/// We extract just the path component (everything up to the first `(`, `<`,
/// or whitespace after the last `::` segment) to match against ledger entries.
fn extract_path(line: &str) -> Option<&str> {
    // Strip leading visibility / kind keywords.
    let stripped = line
        .trim()
        .trim_start_matches("pub ")
        .trim_start_matches("async ")
        .trim_start_matches("fn ")
        .trim_start_matches("struct ")
        .trim_start_matches("enum ")
        .trim_start_matches("trait ")
        .trim_start_matches("type ")
        .trim_start_matches("const ")
        .trim_start_matches("mod ")
        .trim_start_matches("use ");

    // Must start with `zendriver::` to be in scope.
    if !stripped.starts_with("zendriver::") && stripped != "zendriver" {
        // Could be a re-exported `pub use zendriver::Foo` line; handle below.
        if !line.contains("zendriver::") {
            return None;
        }
    }

    // Take up to first `(`, `<`, ` ` or `[` for the path.
    let end = stripped
        .find(|c: char| matches!(c, '(' | '<' | ' ' | '['))
        .unwrap_or(stripped.len());

    let path = &stripped[..end];
    if path.is_empty() || !path.contains("::") {
        return None;
    }
    Some(path)
}

/// Parse `mcp-coverage-ledger.toml` and return the set of `api` strings.
fn load_ledger(ledger_path: &Path) -> HashSet<String> {
    let content = std::fs::read_to_string(ledger_path)
        .unwrap_or_else(|e| panic!("Could not read {}: {e}", ledger_path.display()));

    let table: toml::Value = content
        .parse()
        .unwrap_or_else(|e| panic!("Could not parse {}: {e}", ledger_path.display()));

    let entries = match table.get("entry") {
        Some(toml::Value::Array(arr)) => arr,
        _ => return HashSet::new(),
    };

    entries
        .iter()
        .filter_map(|entry| {
            entry
                .as_table()
                .and_then(|t| t.get("api"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn new_public_items_are_ledgered() {
    let root = workspace_root();
    let baseline_file = baseline_path(&root);
    let ledger_file = ledger_path(&root);

    let current = current_public_api(&root);

    // ── Bootstrap: no baseline yet → generate + pass ─────────────────────
    let baseline_content = std::fs::read_to_string(&baseline_file).unwrap_or_default();
    if baseline_content.trim().is_empty() {
        println!(
            "NOTE: public-api-baseline.txt is absent or empty. \
             Writing the current public API as the initial baseline.\n\
             Commit this file alongside any ledger entries for the APIs it contains."
        );
        std::fs::write(&baseline_file, current.join("\n") + "\n")
            .unwrap_or_else(|e| panic!("Could not write baseline: {e}"));
        return; // First run seeds the baseline; test passes.
    }

    // ── Normal run: diff against baseline ────────────────────────────────
    let baseline: HashSet<String> = baseline_content.lines().map(str::to_owned).collect();

    // Items present now but absent from the baseline = NEW items.
    let new_items: Vec<String> = current
        .iter()
        .filter(|line| !baseline.contains(*line))
        .cloned()
        .collect();

    if new_items.is_empty() {
        return; // Nothing new; test passes.
    }

    let ledger = load_ledger(&ledger_file);

    let mut missing: Vec<String> = Vec::new();
    for line in &new_items {
        if let Some(path) = extract_path(line) {
            // Check if any ledger entry matches this path.
            // We do a substring match so that ledger entries like
            // `zendriver::tab::Tab::request` match lines that include
            // generic params or signatures after the path.
            let in_ledger = ledger
                .iter()
                .any(|api| path == api.as_str() || path.starts_with(&format!("{api}::")));
            if !in_ledger {
                missing.push(format!("  {line}"));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "\n\n\
        The following NEW public `zendriver` APIs have no MCP coverage decision.\n\
        For each item, either:\n  \
          A) Add an MCP tool and record `covered = \"<tool-name>\"` in\n     \
             crates/zendriver-mcp/mcp-coverage-ledger.toml, OR\n  \
          B) Record `excluded = \"<reason>\"` in\n     \
             crates/zendriver-mcp/mcp-coverage-ledger.toml.\n\
        Then update crates/zendriver-mcp/public-api-baseline.txt (see header comment).\n\n\
        Missing ledger entries:\n{}\n",
        missing.join("\n")
    );
}
