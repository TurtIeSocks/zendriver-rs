//! Fetch a pinned CLDR release and regenerate the stealth crate's locale table.

use locale_gen::{emit_rust, rows_from_cldr};

/// Pinned CLDR release tag. Bump to resync; the sync workflow does this.
const CLDR_TAG: &str = "46.0.0";
const OUT: &str = "crates/zendriver-stealth/src/geo/table.rs";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = format!(
        "https://raw.githubusercontent.com/unicode-org/cldr-json/{CLDR_TAG}/cldr-json/cldr-core/supplemental/territoryInfo.json"
    );
    eprintln!("fetching {url}");
    let v: serde_json::Value = reqwest::blocking::get(&url)?.error_for_status()?.json()?;
    let rows = rows_from_cldr(&v);
    eprintln!("emitting {} countries to {OUT}", rows.len());
    std::fs::write(OUT, emit_rust(CLDR_TAG, &rows))?;
    Ok(())
}
