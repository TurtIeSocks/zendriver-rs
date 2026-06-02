//! Insta snapshot: stable `Persona` JSON wire shape.
//!
//! Run `cargo insta accept --all` after any intentional wire-shape change to
//! update the committed snapshot.

#[test]
fn persona_full_wire_shape() {
    let p = zendriver_stealth::Persona::builder()
        .seed(zendriver_stealth::Seed::from_u64(1))
        .device_memory_gb(8)
        .timezone("UTC")
        .build();
    insta::assert_json_snapshot!(p);
}
