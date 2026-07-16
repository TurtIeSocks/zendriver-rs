//! Offline generator for the vendored CLDR country -> language table.
//! Run via `cargo run -p locale-gen`. NOT published.

use serde_json::Value;

/// One emitted table row: an uppercase country code and its ranked language
/// subtags (dominant first).
#[derive(Debug, PartialEq, Eq)]
pub struct CountryRow {
    pub cc: String,
    pub langs: Vec<String>,
}

/// Keep a language if it is official-ish OR spoken by at least this percent.
const POP_THRESHOLD: f64 = 20.0;

/// Transform CLDR `territoryInfo` JSON into sorted, ranked rows.
pub fn rows_from_cldr(v: &Value) -> Vec<CountryRow> {
    let mut rows = Vec::new();
    let Some(map) = v["supplemental"]["territoryInfo"].as_object() else {
        return rows;
    };
    for (cc, info) in map {
        if cc.len() != 2 || !cc.bytes().all(|b| b.is_ascii_alphabetic()) {
            continue; // skip "001" etc.
        }
        let Some(lp) = info.get("languagePopulation").and_then(Value::as_object) else {
            continue;
        };
        let mut langs: Vec<(String, f64, bool)> = lp
            .iter()
            .filter_map(|(lang, d)| {
                let pop = d
                    .get("_populationPercent")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                let official = d
                    .get("_officialStatus")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.starts_with("official") || s == "de_facto_official");
                if official || pop >= POP_THRESHOLD {
                    let subtag = lang.split('_').next().unwrap_or(lang).to_lowercase();
                    Some((subtag, pop, official))
                } else {
                    None
                }
            })
            .collect();
        langs.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
        });
        let mut seen = std::collections::HashSet::new();
        let ranked: Vec<String> = langs
            .into_iter()
            .map(|(s, _, _)| s)
            .filter(|s| seen.insert(s.clone()))
            .collect();
        if !ranked.is_empty() {
            rows.push(CountryRow {
                cc: cc.to_uppercase(),
                langs: ranked,
            });
        }
    }
    rows.sort_by(|a, b| a.cc.cmp(&b.cc));
    rows
}

/// Curated primary-zone overrides for large multi-timezone countries whose
/// `zone1970.tab` first row is not the most-populous zone. Kept minimal;
/// each entry must be verified against a real `zone1970.tab`, never guessed.
///
/// - `RU`: first row is `Europe/Kaliningrad` (~1M); Moscow time covers the
///   large majority of Russia's population.
/// - `UA`: first row is the shared `RU,UA` `Europe/Simferopol` (Crimea,
///   ~2M); mainland Ukraine (`Europe/Kyiv`) is ~40M+.
/// - `CA`: first row is `America/St_Johns` (Newfoundland, ~500K); Ontario +
///   Quebec (`America/Toronto`) hold the bulk of Canada's population.
/// - `AU`: first row is `Australia/Lord_Howe` (~350 residents); New South
///   Wales (`Australia/Sydney`) is Australia's most populous zone.
/// - `BR`: first row is `America/Noronha` (~3K residents); southeast Brazil
///   (`America/Sao_Paulo`) holds the large majority of Brazil's population.
const PRIMARY_TZ_OVERRIDES: &[(&str, &str)] = &[
    ("RU", "Europe/Moscow"),
    ("UA", "Europe/Kyiv"),
    ("CA", "America/Toronto"),
    ("AU", "Australia/Sydney"),
    ("BR", "America/Sao_Paulo"),
    // Add others ONLY where the parsed first-occurrence is demonstrably not
    // the most-populous zone; verify against a real zone1970.tab before adding.
];

/// Parse `zone1970.tab` into a sorted `(country, IANA zone)` list: each
/// country's FIRST-appearing row wins, then `PRIMARY_TZ_OVERRIDES` are applied.
pub fn tz_rows_from_zone_tab(raw: &str) -> Vec<(String, String)> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for line in raw.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let codes = cols.next().unwrap_or("");
        let _coords = cols.next();
        let zone = cols.next().unwrap_or("").trim();
        if zone.is_empty() {
            continue;
        }
        for cc in codes.split(',') {
            map.entry(cc.to_string())
                .or_insert_with(|| zone.to_string()); // first wins
        }
    }
    for (cc, z) in PRIMARY_TZ_OVERRIDES {
        map.insert((*cc).to_string(), (*z).to_string());
    }
    map.into_iter().collect() // BTreeMap -> already sorted by country
}

/// Emit the `table.rs` source for the stealth crate: the CLDR-derived
/// country->language table and the IANA-derived country->timezone table,
/// in one generated file body.
pub fn emit_rust(
    cldr_version: &str,
    rows: &[CountryRow],
    tzdata_version: &str,
    tz_rows: &[(String, String)],
) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "// CLDR {cldr_version}, tzdata {tzdata_version}. Generated by `cargo run -p locale-gen`. DO NOT EDIT.\n"
    ));
    s.push_str(&format!(
        "pub(crate) static CLDR_VERSION: &str = \"{cldr_version}\";\n\n"
    ));
    s.push_str("/// Sorted by country code (binary-searchable). Ranked language subtags, dominant first.\n");
    s.push_str("pub(crate) static COUNTRIES: &[(&str, &[&str])] = &[\n");
    for r in rows {
        let langs = r
            .langs
            .iter()
            .map(|l| format!("\"{l}\""))
            .collect::<Vec<_>>()
            .join(", ");
        s.push_str(&format!("    (\"{}\", &[{}]),\n", r.cc, langs));
    }
    s.push_str("];\n\n");
    s.push_str(&format!(
        "pub(crate) static TZDATA_VERSION: &str = \"{tzdata_version}\";\n\n"
    ));
    s.push_str(
        "/// Sorted by country code (binary-searchable). Representative IANA zone per country.\n",
    );
    s.push_str("pub(crate) static TIMEZONES: &[(&str, &str)] = &[\n");
    for (cc, zone) in tz_rows {
        s.push_str(&format!("    (\"{cc}\", \"{zone}\"),\n"));
    }
    s.push_str("];\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/territoryInfo-mini.json")).unwrap()
    }

    const TZ_FIXTURE: &str = include_str!("../tests/fixtures/zone1970-mini.tab");

    fn lookup<'a>(rows: &'a [(String, String)], cc: &str) -> Option<&'a str> {
        rows.iter().find(|(c, _)| c == cc).map(|(_, z)| z.as_str())
    }

    #[test]
    fn tz_first_occurrence_per_country() {
        // fixture rows: "US ... America/New_York", "US ... America/Chicago" -> US = New_York
        let rows = tz_rows_from_zone_tab(TZ_FIXTURE);
        assert_eq!(lookup(&rows, "US"), Some("America/New_York"));
        assert_eq!(lookup(&rows, "FR"), Some("Europe/Paris"));
    }

    #[test]
    fn tz_override_wins() {
        // fixture has RU first row = Europe/Kaliningrad; override forces Moscow
        let rows = tz_rows_from_zone_tab(TZ_FIXTURE);
        assert_eq!(lookup(&rows, "RU"), Some("Europe/Moscow"));
    }

    #[test]
    fn tz_rows_sorted() {
        let rows = tz_rows_from_zone_tab(TZ_FIXTURE);
        assert!(rows.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn rows_skip_world_region_and_rank() {
        let rows = rows_from_cldr(&fixture());
        let ccs: Vec<&str> = rows.iter().map(|r| r.cc.as_str()).collect();
        assert_eq!(ccs, vec!["CH", "US"]); // sorted, "001" skipped
        let us = rows.iter().find(|r| r.cc == "US").unwrap();
        assert_eq!(us.langs, vec!["en"]); // es below threshold (9.6 < 20, not official)
        let ch = rows.iter().find(|r| r.cc == "CH").unwrap();
        assert_eq!(ch.langs[0], "de"); // dominant official first
        assert!(ch.langs.contains(&"fr".to_string()));
        assert!(ch.langs.contains(&"it".to_string()));
    }

    #[test]
    fn emit_is_valid_rust_shape() {
        let rows = rows_from_cldr(&fixture());
        let tz_rows = tz_rows_from_zone_tab(TZ_FIXTURE);
        let out = emit_rust("46.0.0", &rows, "2026c", &tz_rows);
        assert!(out.contains("CLDR_VERSION: &str = \"46.0.0\""));
        assert!(out.contains("(\"US\", &[\"en\"]),"));
        assert!(out.contains("(\"CH\", &["));
        assert!(out.contains("TZDATA_VERSION: &str = \"2026c\""));
        assert!(out.contains("(\"US\", \"America/New_York\"),"));
        assert!(out.contains("(\"RU\", \"Europe/Moscow\"),"));
    }
}
