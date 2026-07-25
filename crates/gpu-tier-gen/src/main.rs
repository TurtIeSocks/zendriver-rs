//! Regenerate the stealth crate's GPU capability tier tables from the
//! committed probe captures.

use gpu_tier_gen::{TierData, emit_rust, tier_from_capture};

const CAPTURES: &[(&str, &str)] = &[
    (
        "swiftshader",
        "crates/zendriver-stealth/data/gpu-tiers/swiftshader.json",
    ),
    (
        "metal-apple-family3",
        "crates/zendriver-stealth/data/gpu-tiers/metal-apple-family3.json",
    ),
];
const OUT: &str = "crates/zendriver-stealth/src/gpu/tiers.rs";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut tiers: Vec<TierData> = Vec::new();
    for (name, path) in CAPTURES {
        let raw = std::fs::read_to_string(path)?;
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        let prov = v["provenance"].as_str().unwrap_or("unknown");
        tiers.push(tier_from_capture(name, prov, &v["capture"]));
    }
    eprintln!("emitting {} tiers to {OUT}", tiers.len());
    std::fs::write(OUT, emit_rust(&tiers))?;

    // `emit_rust` writes one entry per line for readability, but for a short
    // enough list (few overrides, few extensions) that is not what `cargo
    // fmt` would produce, so the committed file must run through rustfmt
    // itself rather than trust the emitter's raw layout to already be
    // canonical. Doing it here, not by hand after the fact, is what keeps
    // regeneration byte-for-byte stable.
    let status = std::process::Command::new("rustfmt").arg(OUT).status()?;
    if !status.success() {
        return Err(format!("rustfmt {OUT} exited with {status}").into());
    }
    Ok(())
}
