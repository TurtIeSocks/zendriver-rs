//! Regenerate the GPU device catalogue.
//!
//! Run from the workspace root:
//!
//! ```text
//! cargo run -p gpu-catalogue-gen && cargo fmt -p zendriver-stealth
//! ```
//!
//! Both inputs are fetched at the commits pinned in [`gpu_catalogue_gen::sources`],
//! so this needs network access. That is why the regeneration check lives in a
//! scheduled workflow rather than in PR CI, unlike `gpu-tier-gen`, whose inputs
//! are committed captures and can therefore be diffed offline.

use gpu_catalogue_gen::sources::{CORPUS_COMMIT, PCI_IDS_COMMIT, corpus_url, pci_ids_url};

const OUT: &str = "crates/zendriver-stealth/src/gpu/catalogue.rs";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // reqwest 0.13 (`rustls-no-provider`) needs a crypto provider before any Client is built.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let pci_ids = reqwest::blocking::get(pci_ids_url())?
        .error_for_status()?
        .text()?;
    let zipped = reqwest::blocking::get(corpus_url())?
        .error_for_status()?
        .bytes()?;
    let network = gpu_catalogue_gen::unzip_network_json(&zipped)?;

    let report = gpu_catalogue_gen::build_catalogue(&network, &pci_ids);
    std::fs::write(
        OUT,
        gpu_catalogue_gen::emit_rust(CORPUS_COMMIT, PCI_IDS_COMMIT, &report),
    )?;

    // Say what was dropped. A catalogue that quietly shrinks reads exactly like
    // one that did not, and the whole point of dropping rather than inventing a
    // device id is that the loss stays visible.
    println!("emitted {} entries to {OUT}", report.rows.len());
    if !report.unmatched.is_empty() {
        println!(
            "dropped {} models with no device id in either source:",
            report.unmatched.len()
        );
        for model in &report.unmatched {
            println!("  {model}");
        }
    }
    Ok(())
}
