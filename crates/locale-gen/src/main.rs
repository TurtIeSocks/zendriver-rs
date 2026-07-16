//! Fetch a pinned CLDR release + a pinned IANA tzdata release and regenerate
//! the stealth crate's locale/timezone table.

use locale_gen::{emit_rust, rows_from_cldr, tz_rows_from_zone_tab};

/// Pinned CLDR release tag. Bump to resync; the sync workflow does this.
const CLDR_TAG: &str = "46.0.0";
/// Pinned `eggert/tz` release tag `zone1970.tab` is fetched from. Bump to
/// resync; the sync workflow does this.
const TZDATA_TAG: &str = "2026c";
const OUT: &str = "crates/zendriver-stealth/src/geo/table.rs";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cldr_url = format!(
        "https://raw.githubusercontent.com/unicode-org/cldr-json/{CLDR_TAG}/cldr-json/cldr-core/supplemental/territoryInfo.json"
    );
    eprintln!("fetching {cldr_url}");
    let v: serde_json::Value = reqwest::blocking::get(&cldr_url)?
        .error_for_status()?
        .json()?;
    let rows = rows_from_cldr(&v);

    let tz_url = format!("https://raw.githubusercontent.com/eggert/tz/{TZDATA_TAG}/zone1970.tab");
    eprintln!("fetching {tz_url}");
    let raw = reqwest::blocking::get(&tz_url)?
        .error_for_status()?
        .text()?;
    let tz_rows = tz_rows_from_zone_tab(&raw);

    eprintln!(
        "emitting {} countries and {} timezones to {OUT}",
        rows.len(),
        tz_rows.len()
    );
    std::fs::write(OUT, emit_rust(CLDR_TAG, &rows, TZDATA_TAG, &tz_rows))?;
    Ok(())
}
