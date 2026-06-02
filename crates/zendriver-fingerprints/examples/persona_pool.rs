//! Illustrates building a `PoolSet`, sampling a `Persona` by seed, and
//! inspecting the result.
//!
//! In real usage you would call `pool::load_or_download(url)` to fetch a
//! large real-device dataset from a CDN; this example uses a small in-memory
//! set so it runs offline.
//!
//! The resulting `Persona` value can be passed directly to
//! `Browser::builder().persona(p)` in the `zendriver` crate (not a dep here).
//!
//! Run with:
//! ```sh
//! cargo run -p zendriver-fingerprints --example persona_pool --features pool
//! ```

use zendriver_fingerprints::pool::PoolSet;
use zendriver_stealth::{Persona, Platform, Seed};

fn make_persona(platform: Platform, memory_gb: u32) -> Persona {
    Persona {
        platform: Some(platform),
        device_memory_gb: Some(memory_gb),
        ..Persona::default()
    }
}

fn main() {
    // Build a small in-memory pool of real-device personas.
    let pool = PoolSet::from_records(vec![
        make_persona(Platform::Win32, 8),
        make_persona(Platform::MacIntel, 16),
        make_persona(Platform::Win32, 4),
        make_persona(Platform::LinuxX86_64, 8),
    ]);

    // Sample deterministically — same seed always picks the same entry.
    let seed = Seed::from_u64(42);
    let persona = pool.sample(seed);

    println!("Sampled persona:");
    println!("  platform         = {:?}", persona.platform.unwrap());
    println!(
        "  device_memory_gb = {:?}",
        persona.device_memory_gb.unwrap()
    );

    // Show reproducibility: sampling again with the same seed gives the same result.
    let same_again = pool.sample(seed);
    assert_eq!(
        persona.device_memory_gb, same_again.device_memory_gb,
        "same seed must yield the same persona"
    );
    println!("  (reproducible: same seed → same persona ✓)");

    // In a real application you would then do:
    //   Browser::builder().persona(persona).launch().await?
    // but zendriver is not a dep of zendriver-fingerprints.
    println!("\nPass `persona` to `Browser::builder().persona(persona)` to use it.");
}
