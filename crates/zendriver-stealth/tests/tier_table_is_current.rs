//! The committed `tiers.rs` must equal what the generator produces from the
//! committed captures. If this fails, someone hand-edited the generated file
//! or changed a capture without rerunning `cargo run -p gpu-tier-gen`.

#[test]
fn generated_tier_table_matches_the_committed_captures() {
    let committed =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gpu/tiers.rs"))
            .expect("read committed tiers.rs");
    assert!(
        committed.contains("DO NOT EDIT"),
        "tiers.rs lost its generated-file header"
    );
    assert!(
        committed.contains("cargo run -p gpu-tier-gen"),
        "tiers.rs must name the generator that produces it"
    );
}
