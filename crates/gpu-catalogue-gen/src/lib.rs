//! Generator for the GPU device catalogue.
//!
//! Emits `crates/zendriver-stealth/src/gpu/catalogue.rs` from two pinned
//! sources: driver-reported model names out of the fingerprint corpus, and PCI
//! device IDs out of `pci.ids`. Nothing here invents a value.
//!
//! The catalogue supplies *identity* only — a renderer string, a device ID, an
//! architecture token. Every capability number still comes from the measured
//! tier tables that `gpu-tier-gen` emits, which is what keeps this generator
//! from being able to fabricate a capability at all.

pub mod sources;

/// One `<vendor id>:<device id>` pair and the name `pci.ids` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PciDevice {
    pub vendor_id: u32,
    pub device_id: u32,
    pub name: String,
}

/// Parse the `pci.ids` format.
///
/// The shape is three levels deep, distinguished only by leading tabs:
///
/// ```text
/// 10de  NVIDIA Corporation            <- vendor
/// \t2684  AD102 [GeForce RTX 4090]    <- device
/// \t\t1043 87b3  TUF Gaming           <- subsystem
/// ```
///
/// Only the middle level carries a device ID. Subsystem lines are a
/// `(subvendor, subdevice)` pair, so reading one as a device invents a card
/// that does not exist.
///
/// The file then ends with device-*class* sections, whose headings are
/// unindented like vendors and whose subclass lines are indented exactly like
/// devices:
///
/// ```text
/// C 03  Display controller
/// \t00  VGA compatible controller
/// ```
///
/// `C 03` is not hex, so the current vendor is **cleared** rather than left
/// alone. Leaving it alone would file every subclass in the file under
/// whichever vendor happened to come last.
#[must_use]
pub fn parse_pci_ids(raw: &str) -> Vec<PciDevice> {
    let mut out = Vec::new();
    let mut vendor_id: Option<u32> = None;

    for line in raw.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix('\t') {
            // Two tabs: a subsystem pair, which has no device ID of its own.
            if rest.starts_with('\t') {
                continue;
            }
            let (Some(vendor), Some((id, name))) = (vendor_id, rest.split_once("  ")) else {
                continue;
            };
            if let Ok(device_id) = u32::from_str_radix(id.trim(), 16) {
                out.push(PciDevice {
                    vendor_id: vendor,
                    device_id,
                    name: name.trim().to_string(),
                });
            }
            continue;
        }

        // Unindented: a vendor heading, or a class heading that is not one.
        vendor_id = line
            .split_once("  ")
            .and_then(|(id, _)| u32::from_str_radix(id.trim(), 16).ok());
    }
    out
}

/// Which ANGLE backend composes a renderer string, and therefore which format
/// it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Backend {
    D3d11,
    Metal,
}

/// Build the renderer string ANGLE would report for a device.
///
/// Reconstructed from the source that composes it rather than copied from
/// samples, because the format has changed under the corpus: strings collected
/// on older Chrome omit the device ID current ANGLE always appends. Composing
/// means a future format change is a small, detectable fix here instead of
/// silent rot spread across every row of the data.
///
/// D3D11, `Renderer11.cpp:2308-2319`:
///
/// ```text
/// mDescription << " (" << FmtHex(DeviceId) << ")" << " Direct3D11"
///              << " vs_" << major << "_" << minor << " ps_" << major << "_" << minor
/// ```
///
/// Metal, `DisplayMtl.mm:188-201`:
///
/// ```text
/// "ANGLE Metal Renderer" + ": " + MTLDevice.name
/// ```
///
/// The trailing Metal field is the literal `getVersionString` returns for WebGL
/// contexts (`:216`) because Chrome requires something there. It is not a
/// version and must never be synthesized as one.
///
/// `device_id` is ignored on Metal: Apple silicon exposes no PCI ID, and the
/// string has nowhere to put one.
#[must_use]
pub fn compose_renderer(
    backend: Backend,
    vendor: &str,
    model: &str,
    device_id: Option<u32>,
) -> String {
    match backend {
        Backend::D3d11 => {
            let id = device_id.unwrap_or(0);
            format!("ANGLE ({vendor}, {model} (0x{id:08X}) Direct3D11 vs_5_0 ps_5_0, D3D11)")
        }
        Backend::Metal => {
            format!("ANGLE ({vendor}, ANGLE Metal Renderer: {model}, Unspecified Version)")
        }
    }
}

/// One device the corpus reports. The renderer string itself is rebuilt by
/// [`compose_renderer`].
#[derive(Debug, Clone, PartialEq)]
pub struct CorpusModel {
    pub backend: Backend,
    pub vendor: String,
    pub model: String,
    /// The PCI device id the corpus observed **this name paired with**, when
    /// the string carried one.
    ///
    /// This is an observation, not a lookup: 467 such pairs cover 308 of the
    /// 310 D3D11 names, and 77 names appear with several ids because one
    /// marketing name really does span several SKUs. Preferring these over a
    /// `pci.ids` name match is what turns a deterministic guess among 30
    /// candidates into a pairing some real machine actually reported.
    pub device_id: Option<u32>,
    /// Marginal probability of this device across the whole corpus.
    ///
    /// `videoCard` is conditioned on `userAgent`, so a bucket's numbers are
    /// `P(device | ua)` and summing them naively would over-weight anything
    /// that appears under many user agents. `userAgent` is parentless and its
    /// prior sums to 1, so the honest weight is
    /// `Σ_ua P(ua) · P(device | ua)`.
    pub weight: f64,
}

/// Key that orders [`CorpusModel`]s deterministically. `weight` is an `f64`,
/// so the struct cannot derive `Ord`.
fn model_key(m: &CorpusModel) -> (Backend, &str, &str, Option<u32>) {
    (m.backend, m.vendor.as_str(), m.model.as_str(), m.device_id)
}

/// Reduce a model name to the form both sides of the `pci.ids` join agree on.
///
/// Two mistakes are already paid for here, so do not simplify them away.
/// Writing the `(tm)`/`(r)` strip as `\b\(tm\)\b` matches nothing, because a
/// word boundary before a parenthesis is not where it looks like it is; that
/// alone dropped Intel's join rate to zero. The vendor words have to go too,
/// because the driver leads with them (`NVIDIA GeForce RTX 3070`) and
/// `pci.ids` does not (`GA104 [GeForce RTX 3070]`).
#[must_use]
pub fn normalize_model(s: &str) -> String {
    const DROP_WORDS: &[&str] = &[
        "nvidia",
        "amd",
        "ati",
        "intel",
        "corporation",
        "inc",
        "series",
    ];
    let lower = s
        .to_ascii_lowercase()
        .replace("(tm)", "")
        .replace("(r)", "");
    let mut out = String::new();
    for word in lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if DROP_WORDS.contains(&word) {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Every marketing name one `pci.ids` entry covers.
///
/// A single entry routinely stands for several cards:
/// `Navi 22 [Radeon RX 6700/6700 XT/6750 XT]`. Only the first slash segment
/// carries the `Radeon RX` prefix, so each later segment is tried both bare and
/// with that prefix prepended. Skipping this costs about a third of AMD.
fn pci_aliases(name: &str) -> Vec<String> {
    let mut out = vec![normalize_model(name)];
    let mut rest = name;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else {
            break;
        };
        let bracket = &rest[open + 1..open + close];
        rest = &rest[open + close..];

        out.push(normalize_model(bracket));
        let parts: Vec<&str> = bracket
            .split('/')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() > 1 {
            // "Radeon RX 6700" lends "Radeon RX" to "6700 XT" and "6750 XT".
            let head: Vec<&str> = parts[0].split_whitespace().collect();
            let prefix = head[..head.len().saturating_sub(1)].join(" ");
            for part in &parts {
                out.push(normalize_model(part));
                if !prefix.is_empty() {
                    out.push(normalize_model(&format!("{prefix} {part}")));
                }
            }
        }
    }
    out.retain(|a| !a.is_empty());
    out.sort();
    out.dedup();
    out
}

/// PCI vendor id for each ANGLE vendor token the catalogue keeps.
const VENDOR_IDS: &[(&str, u32)] = &[("NVIDIA", 0x10de), ("AMD", 0x1002), ("Intel", 0x8086)];

/// A catalogue row on its way to the generated file.
///
/// Strings rather than the stealth crate's enums, because this generator does
/// not depend on it: `tier` is rendered as `Tier::{tier}`.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogueRow {
    pub model: String,
    pub vendor: String,
    pub device_id: Option<u32>,
    pub tier: String,
    /// Marginal share of this device in the corpus population, for the
    /// share-weighted draw. Browser population rather than gamer population,
    /// which is the one a browser-automation tool wants.
    pub weight: f64,
    /// Where the device id came from. Nothing is emitted without one on
    /// D3D11, so this records which of the two grounded sources supplied it.
    pub id_source: IdSource,
}

/// Provenance of a row's device id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdSource {
    /// The corpus reported this exact `(name, id)` pair. Preferred: it is an
    /// observation of a real machine rather than a reconstruction.
    Observed,
    /// No corpus entry for this name carried an id, so it was resolved by
    /// name against `pci.ids`. Rare — 2 of 310 names.
    PciIds,
    /// Metal, which has no PCI id at all.
    None,
}

/// What a `build_catalogue` run kept and what it had to drop.
#[derive(Debug, Clone, Default)]
pub struct BuildReport {
    pub rows: Vec<CatalogueRow>,
    /// D3D11 models with no `pci.ids` match, so no device ID, so no composable
    /// renderer string. Reported rather than silently swallowed.
    pub unmatched: Vec<String>,
}

/// Join corpus models against `pci.ids` and produce the catalogue rows.
///
/// A D3D11 renderer string carries its device ID by construction, so a model
/// that fails to join **cannot be composed at all** and is dropped. Inventing
/// the number is the one thing this design forbids, and a placeholder would be
/// a fabricated fingerprint value wearing a real card's name.
///
/// Metal needs no join: Apple silicon exposes no PCI ID and the string has
/// nowhere to put one.
#[must_use]
pub fn build_catalogue(network_json: &str, pci_ids: &str) -> BuildReport {
    let devices = parse_pci_ids(pci_ids);
    let mut by_vendor: std::collections::BTreeMap<u32, Vec<(&PciDevice, Vec<String>)>> =
        std::collections::BTreeMap::new();
    for d in &devices {
        if VENDOR_IDS.iter().any(|(_, id)| *id == d.vendor_id) {
            by_vendor
                .entry(d.vendor_id)
                .or_default()
                .push((d, pci_aliases(&d.name)));
        }
    }

    let mut report = BuildReport::default();
    for m in extract_models(network_json) {
        // The one measured vendor split: ANGLE applies
        // skipVSConstantRegisterZero when and only when isNvidia.
        let tier = match (m.backend, m.vendor.as_str()) {
            (Backend::Metal, _) => "MetalMacos",
            (Backend::D3d11, "NVIDIA") => "D3d11Fl11Nvidia",
            (Backend::D3d11, _) => "D3d11Fl11",
        };

        if m.backend == Backend::Metal {
            report.rows.push(CatalogueRow {
                model: m.model,
                vendor: m.vendor,
                device_id: None,
                tier: tier.to_string(),
                weight: m.weight,
                id_source: IdSource::None,
            });
            continue;
        }

        // Prefer what the corpus observed. A pci.ids name match can only
        // reconstruct a plausible id -- `Intel(R) Graphics` alone matches 30
        // candidates -- whereas the corpus reports the pair a real machine
        // actually emitted, and reports every SKU a name spans rather than
        // one representative of them.
        if let Some(device_id) = m.device_id {
            report.rows.push(CatalogueRow {
                model: m.model,
                vendor: m.vendor,
                device_id: Some(device_id),
                tier: tier.to_string(),
                weight: m.weight,
                id_source: IdSource::Observed,
            });
            continue;
        }

        // Fallback for the few names no corpus entry ever paired with an id.
        // Lowest id, so bumping the pinned pci.ids cannot reshuffle it.
        let vendor_id = VENDOR_IDS
            .iter()
            .find(|(v, _)| *v == m.vendor)
            .map(|(_, id)| id);
        let target = normalize_model(&m.model);
        let hit = vendor_id
            .and_then(|id| by_vendor.get(id))
            .into_iter()
            .flatten()
            .filter(|(_, aliases)| !target.is_empty() && aliases.contains(&target))
            .min_by_key(|(device, _)| device.device_id);
        match hit {
            Some((device, _)) => report.rows.push(CatalogueRow {
                model: m.model,
                vendor: m.vendor,
                device_id: Some(device.device_id),
                tier: tier.to_string(),
                weight: m.weight,
                id_source: IdSource::PciIds,
            }),
            // A D3D11 renderer string carries its device id by construction,
            // so a row without one cannot be composed at all. Dropped, named.
            None => report.unmatched.push(m.model),
        }
    }
    report
        .rows
        .sort_by(|a, b| (&a.tier, &a.model, a.device_id).cmp(&(&b.tier, &b.model, b.device_id)));
    report.unmatched.sort();
    report.unmatched.dedup();
    report
}

/// Vendors whose D3D11 parts are out of scope, with the reason each is out.
///
/// These are not hypothetical: every one appears in the pinned corpus.
const D3D11_VENDOR_EXCLUSIONS: &[(&str, &str)] = &[
    // Windows-on-ARM. The non-NVIDIA D3D11 tier's numbers were measured on an
    // x86 AMD part, and nothing has probed an Adreno under D3D11, so filing
    // one under that tier would claim a generalization across instruction set
    // and vendor at once.
    (
        "Qualcomm",
        "Windows-on-ARM, and no ARM D3D11 capture exists",
    ),
    // WARP, a software rasterizer wearing a D3D11 renderer string. Its numbers
    // are not a GPU's, which is the same reason SwiftShader is its own tier.
    (
        "Microsoft",
        "Microsoft Basic Render Driver is WARP, not a GPU",
    ),
];

/// Apple silicon prefixes that are **not** Macs.
///
/// The Metal tier is ANGLE's `TARGET_OS_OSX` arm, whose caps are compile-time
/// constants. iOS takes the `#else` arm, where `supportsAppleGPUFamily` picks
/// different ones, so an A-series identity over macOS numbers is the exact
/// backend mismatch the tiers exist to prevent.
const NON_MAC_APPLE_PREFIXES: &[&str] = &["Apple A"];

/// Pull every in-scope `(backend, vendor, model)` out of the corpus's
/// `videoCard` node.
///
/// Only the model name is taken. The renderer string is rebuilt by
/// [`compose_renderer`], because the corpus predates the device ID current
/// ANGLE appends and a copied string would carry whichever Chrome collected it.
///
/// Results are sorted and deduplicated, so the emitted catalogue does not
/// change when the corpus reorders its buckets.
#[must_use]
pub fn extract_models(network_json: &str) -> Vec<CorpusModel> {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(network_json) else {
        return Vec::new();
    };
    let nodes = root["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[]);

    // `userAgent` is parentless, so its conditional table is a plain prior.
    let mut prior: std::collections::BTreeMap<&str, f64> = std::collections::BTreeMap::new();
    for node in nodes {
        if node["name"] != "userAgent" {
            continue;
        }
        for (ua, p) in node["conditionalProbabilities"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if let Some(p) = p.as_f64() {
                prior.insert(ua.as_str(), p);
            }
        }
    }

    let mut totals: std::collections::BTreeMap<(Backend, String, String, Option<u32>), f64> =
        std::collections::BTreeMap::new();

    for node in nodes {
        if node["name"] != "videoCard" {
            continue;
        }
        let deeper = &node["conditionalProbabilities"]["deeper"];
        for (ua, bucket) in deeper.as_object().into_iter().flatten() {
            // A user agent the prior does not mention contributes nothing: its
            // marginal weight is zero, so the device is still catalogued but
            // never drawn by share.
            let p_ua = prior.get(ua.as_str()).copied().unwrap_or(0.0);
            for (key, p) in bucket.as_object().into_iter().flatten() {
                let json = key.strip_prefix("*STRINGIFIED*").unwrap_or(key);
                let Ok(card) = serde_json::from_str::<serde_json::Value>(json) else {
                    continue;
                };
                let Some(renderer) = card["renderer"].as_str() else {
                    continue;
                };
                let Some(m) = split_renderer(renderer) else {
                    continue;
                };
                *totals
                    .entry((m.backend, m.vendor, m.model, m.device_id))
                    .or_default() += p_ua * p.as_f64().unwrap_or(0.0);
            }
        }
    }

    let mut out: Vec<CorpusModel> = totals
        .into_iter()
        .map(
            |((backend, vendor, model, device_id), weight)| CorpusModel {
                backend,
                vendor,
                model,
                device_id,
                weight,
            },
        )
        .collect();
    out.sort_by(|a, b| model_key(a).cmp(&model_key(b)));
    out
}

/// Take an ANGLE renderer string apart, or answer `None` when it describes
/// something v1 does not model.
fn split_renderer(renderer: &str) -> Option<CorpusModel> {
    let inner = renderer.strip_prefix("ANGLE (")?.strip_suffix(')')?;
    let (vendor, rest) = inner.split_once(", ")?;

    // A vendor ANGLE could not name, so it printed the raw PCI id — a VM
    // display adapter in every corpus instance. There is no real device here.
    if vendor.starts_with("0x") {
        return None;
    }

    if let Some(model) = rest.strip_prefix("ANGLE Metal Renderer: ") {
        // Everything before the trailing ", Unspecified Version" field.
        let model = model.split_once(", ").map_or(model, |(before, _)| before);
        if NON_MAC_APPLE_PREFIXES.iter().any(|p| model.starts_with(p)) {
            return None;
        }
        // Intel and AMD Macs are deliberately out of scope for v1: Dawn
        // resolves their WebGPU architecture through a `gpu_info` lookup
        // rather than the `mDeviceId == 0` path every Apple silicon part
        // takes, and nothing has probed one. Dropping them costs identities;
        // guessing an architecture token costs coherence.
        if vendor != "Apple" {
            return None;
        }
        return Some(CorpusModel {
            backend: Backend::Metal,
            vendor: vendor.to_string(),
            model: model.to_string(),
            device_id: None,
            weight: 0.0,
        });
    }

    if let Some(body) = rest.strip_suffix(", D3D11") {
        if D3D11_VENDOR_EXCLUSIONS.iter().any(|(v, _)| *v == vendor) {
            return None;
        }
        let named = body.split(" Direct3D11").next()?.trim();
        // Keep the device id when the string carries one. Chrome has appended
        // it for some time, so most entries have it, and an observed pairing
        // beats anything a name lookup can reconstruct: one marketing name
        // spans many SKUs, and the corpus knows which ones actually occur.
        let (model, device_id) = match named.rsplit_once(" (0x") {
            Some((before, hex)) => {
                let hex = hex.strip_suffix(')').unwrap_or(hex);
                (before.trim(), u32::from_str_radix(hex, 16).ok())
            }
            None => (named, None),
        };
        if model.is_empty() {
            return None;
        }
        return Some(CorpusModel {
            backend: Backend::D3d11,
            vendor: vendor.to_string(),
            model: model.to_string(),
            device_id,
            weight: 0.0,
        });
    }

    // Every other backend: desktop GL, GLES, Vulkan. Real, but device-derived
    // or unmodelled, so out of scope for a catalogue built on shared tiers.
    None
}

#[cfg(test)]
mod join_tests {
    use super::*;

    const CORPUS: &str = include_str!("../tests/fixtures/corpus-mini.json");
    const PCI: &str = include_str!("../tests/fixtures/pci-mini.ids");

    #[test]
    fn normalization_survives_the_parenthesised_trademarks() {
        // Written as a \b-anchored regex this matches nothing, because a word
        // boundary before "(" is not where it looks like it is. That mistake
        // took Intel's join rate to zero.
        assert_eq!(
            normalize_model("Intel(R) UHD Graphics 630"),
            "uhd graphics 630"
        );
        assert_eq!(
            normalize_model("AMD Radeon(TM) Graphics"),
            "radeon graphics"
        );
        assert_eq!(
            normalize_model("NVIDIA GeForce RTX 3070"),
            normalize_model("GeForce RTX 3070"),
            "the driver leads with the vendor and pci.ids does not"
        );
    }

    #[test]
    fn a_slash_group_lends_its_prefix_to_every_segment() {
        let aliases = pci_aliases("Navi 22 [Radeon RX 6700/6700 XT/6750 XT]");
        for want in ["radeon rx 6700", "radeon rx 6700 xt", "radeon rx 6750 xt"] {
            assert!(
                aliases.contains(&want.to_string()),
                "{want} missing from {aliases:?}"
            );
        }
    }

    #[test]
    fn joins_a_driver_name_to_its_device_id() {
        let report = build_catalogue(CORPUS, PCI);
        let rtx = report
            .rows
            .iter()
            .find(|r| r.model == "NVIDIA GeForce RTX 4090")
            .expect("RTX 4090 row");
        assert_eq!(rtx.device_id, Some(0x2684));
        assert_eq!(rtx.tier, "D3d11Fl11Nvidia");

        let uhd = report
            .rows
            .iter()
            .find(|r| r.model == "Intel(R) UHD Graphics 630")
            .expect("UHD 630 row");
        assert_eq!(uhd.device_id, Some(0x3e92));
        assert_eq!(
            uhd.tier, "D3d11Fl11",
            "only NVIDIA takes the workaround tier"
        );
    }

    #[test]
    fn a_metal_row_needs_no_join_and_carries_no_device_id() {
        let report = build_catalogue(CORPUS, PCI);
        let m2 = report
            .rows
            .iter()
            .find(|r| r.model == "Apple M2")
            .expect("M2 row");
        assert_eq!(m2.device_id, None);
        assert_eq!(m2.tier, "MetalMacos");
    }

    #[test]
    fn an_unjoinable_model_is_dropped_and_reported_never_given_a_placeholder_id() {
        // "AMD Radeon RX 6700 XT" is in the corpus fixture; the pci.ids fixture
        // has the slash-grouped Navi 22 entry, so it joins. Nothing in the
        // fixtures should end up with a fabricated id either way.
        let report = build_catalogue(CORPUS, PCI);
        assert!(
            report
                .rows
                .iter()
                .all(|r| r.tier == "MetalMacos" || r.device_id.is_some()),
            "a D3D11 row without a device id cannot compose a renderer string: {:?}",
            report.rows
        );
        // Whatever failed to join is named, so a shrinking catalogue is visible
        // rather than silent.
        assert!(
            report.unmatched.iter().all(|m| !m.is_empty()),
            "{:?}",
            report.unmatched
        );
    }

    #[test]
    fn an_ambiguous_name_resolves_to_the_lowest_device_id() {
        // The pci.ids fixture gives NVIDIA both 0x2484 and 0x2684. A model
        // whose normalized name matched both must land on 0x2484 by rule
        // rather than by whichever line the file listed first, so bumping the
        // pinned pci.ids commit cannot reshuffle the catalogue underneath us.
        let pci = "10de  NVIDIA Corporation\n\
                   \t2684  AD102 [GeForce Ambiguous]\n\
                   \t2484  GA104 [GeForce Ambiguous]\n";
        let corpus = r#"{"nodes":[{"name":"videoCard","conditionalProbabilities":{"deeper":{"x":{
          "*STRINGIFIED*{\"renderer\":\"ANGLE (NVIDIA, NVIDIA GeForce Ambiguous Direct3D11 vs_5_0 ps_5_0, D3D11)\",\"vendor\":\"\"}": 1.0}}}}]}"#;
        let report = build_catalogue(corpus, pci);
        assert_eq!(report.rows.len(), 1, "{report:?}");
        assert_eq!(report.rows[0].device_id, Some(0x2484));
    }

    #[test]
    fn every_row_composes_a_renderer_string_that_names_its_own_device() {
        let report = build_catalogue(CORPUS, PCI);
        for row in &report.rows {
            let backend = if row.tier == "MetalMacos" {
                Backend::Metal
            } else {
                Backend::D3d11
            };
            let s = compose_renderer(backend, &row.vendor, &row.model, row.device_id);
            assert!(s.contains(&row.model), "{s} does not name {}", row.model);
            assert!(s.starts_with(&format!("ANGLE ({}, ", row.vendor)), "{s}");
        }
    }
}

#[cfg(test)]
mod extract_tests {
    use super::*;

    const MINI: &str = include_str!("../tests/fixtures/corpus-mini.json");

    fn models() -> Vec<CorpusModel> {
        extract_models(MINI)
    }

    fn has(models: &[CorpusModel], model: &str) -> bool {
        models.iter().any(|m| m.model == model)
    }

    #[test]
    fn keeps_the_in_scope_devices() {
        let m = models();
        assert!(has(&m, "NVIDIA GeForce RTX 3060"));
        assert!(has(&m, "AMD Radeon RX 6700 XT"));
        assert!(has(&m, "Intel(R) UHD Graphics 630"));
        assert!(has(&m, "Apple M2"));
    }

    #[test]
    fn strips_a_device_id_from_a_newer_corpus_entry() {
        // Only the name is taken; the ID is resolved from pci.ids. A name that
        // kept "(0x00002684)" would match nothing there.
        let m = models();
        assert!(has(&m, "NVIDIA GeForce RTX 4090"), "{m:?}");
        assert!(!m.iter().any(|x| x.model.contains("0x")), "{m:?}");
    }

    #[test]
    fn drops_every_out_of_scope_device() {
        let m = models();
        for (model, why) in [
            ("Apple A18 Pro", "iOS takes ANGLE's non-macOS arm"),
            ("Qualcomm(R) Adreno(TM) X1-85 GPU", "Windows-on-ARM"),
            ("Microsoft Basic Render Driver", "WARP, not a GPU"),
            ("Parallels Display Adapter (WDDM)", "VM adapter, hex vendor"),
            ("Adreno (TM) 730", "GLES, not a modelled backend"),
            ("llvmpipe", "desktop GL software rasterizer"),
        ] {
            assert!(!has(&m, model), "{model} must be dropped: {why}");
        }
    }

    #[test]
    fn drops_intel_and_amd_macs_while_v1_has_no_capture_for_them() {
        // Flip this the day a probe from one exists; the tier's caps very
        // likely cover them, but the WebGPU architecture token does not.
        let m = models();
        assert!(!has(&m, "Intel(R) UHD Graphics 630 "), "{m:?}");
        assert!(!has(&m, "AMD Radeon Pro 5500M"), "{m:?}");
        assert!(
            m.iter()
                .filter(|x| x.backend == Backend::Metal)
                .all(|x| x.vendor == "Apple"),
            "every Metal entry must be Apple silicon in v1: {m:?}"
        );
    }

    #[test]
    fn output_is_sorted_and_deduplicated() {
        let m = models();
        let keys: Vec<_> = m
            .iter()
            .map(|x| (x.backend, x.vendor.clone(), x.model.clone(), x.device_id))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            keys, sorted,
            "emitted order must not depend on corpus bucket order"
        );
    }

    #[test]
    fn a_device_id_in_the_string_is_kept_not_discarded() {
        // The corpus is the better source for these: it reports the pair a
        // real machine emitted, where a pci.ids name lookup can only pick one
        // plausible SKU out of the many sharing a marketing name.
        let m = models();
        let rtx = m
            .iter()
            .find(|x| x.model == "NVIDIA GeForce RTX 4090")
            .expect("4090");
        assert_eq!(rtx.device_id, Some(0x2684));
        // ...while the name itself must not keep the id text.
        assert!(!m.iter().any(|x| x.model.contains("0x")), "{m:?}");
    }

    #[test]
    fn weights_marginalize_over_the_user_agent_prior() {
        // videoCard is conditioned on userAgent, so a raw sum over buckets
        // over-weights whatever is common to many user agents. Each weight is
        // P(ua) * P(card|ua) accumulated, so the total is a probability mass.
        let total: f64 = models().iter().map(|x| x.weight).sum();
        assert!(
            total > 0.0 && total <= 1.0 + 1e-9,
            "marginal weights must be a probability mass, got {total}"
        );
    }
}

#[cfg(test)]
mod compose_tests {
    use super::*;

    // Read the expected strings out of the committed captures rather than
    // restating them here. A hand-copied literal would pass even if the
    // captures and the composer drifted apart, which is the only thing this
    // test exists to catch.
    const NVIDIA: &str =
        include_str!("../../zendriver-stealth/data/gpu-tiers/d3d11-fl11-nvidia.json");
    const AMD: &str = include_str!("../../zendriver-stealth/data/gpu-tiers/d3d11-fl11.json");
    const METAL: &str = include_str!("../../zendriver-stealth/data/gpu-tiers/metal-macos.json");

    fn captured_renderer(raw: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(raw).expect("capture json");
        v["capture"]["webgl2"]["unmaskedRenderer"]
            .as_str()
            .expect("unmaskedRenderer")
            .to_string()
    }

    #[test]
    fn composition_reproduces_every_committed_capture() {
        assert_eq!(
            compose_renderer(
                Backend::D3d11,
                "NVIDIA",
                "NVIDIA GeForce RTX 4090",
                Some(0x2684)
            ),
            captured_renderer(NVIDIA)
        );
        assert_eq!(
            compose_renderer(
                Backend::D3d11,
                "AMD",
                "AMD Radeon(TM) Graphics",
                Some(0x164E)
            ),
            captured_renderer(AMD)
        );
        assert_eq!(
            compose_renderer(Backend::Metal, "Apple", "Apple M4 Pro", None),
            captured_renderer(METAL)
        );
    }

    #[test]
    fn the_device_id_is_zero_padded_to_eight_uppercase_hex_digits() {
        // FmtHex writes the full 32-bit field: 0x2684 is "(0x00002684)", not
        // "(0x2684)". Getting the width wrong produces a string that reads as
        // plausible and matches no device Chrome has ever reported.
        let s = compose_renderer(Backend::D3d11, "NVIDIA", "X", Some(0x2684));
        assert!(s.contains("(0x00002684)"), "{s}");
        let low = compose_renderer(Backend::D3d11, "AMD", "X", Some(0x9));
        assert!(low.contains("(0x00000009)"), "{low}");
        // Lowercase hex would be wrong too: 0x164E, not 0x164e.
        let amd = compose_renderer(Backend::D3d11, "AMD", "X", Some(0x164E));
        assert!(amd.contains("(0x0000164E)"), "{amd}");
    }

    #[test]
    fn metal_ignores_a_device_id_it_has_nowhere_to_put() {
        let with = compose_renderer(Backend::Metal, "Apple", "Apple M2", Some(0x1234));
        let without = compose_renderer(Backend::Metal, "Apple", "Apple M2", None);
        assert_eq!(with, without);
        assert!(!with.contains("0x"), "{with}");
    }
}

#[cfg(test)]
mod pci_tests {
    use super::*;

    const MINI: &str = include_str!("../tests/fixtures/pci-mini.ids");

    #[test]
    fn parses_vendor_and_device_lines() {
        let devices = parse_pci_ids(MINI);
        let rtx4090 = devices
            .iter()
            .find(|d| d.device_id == 0x2684)
            .expect("RTX 4090 device line");
        assert_eq!(rtx4090.vendor_id, 0x10de);
        assert_eq!(rtx4090.name, "AD102 [GeForce RTX 4090]");
    }

    #[test]
    fn keeps_each_device_under_its_own_vendor() {
        let devices = parse_pci_ids(MINI);
        // The bug this guards: carrying the previous vendor across a new
        // unindented line would file Raphael under NVIDIA.
        let raphael = devices.iter().find(|d| d.device_id == 0x164e).unwrap();
        assert_eq!(raphael.vendor_id, 0x1002);
        let uhd = devices.iter().find(|d| d.device_id == 0x3e92).unwrap();
        assert_eq!(uhd.vendor_id, 0x8086);
    }

    #[test]
    fn skips_subsystem_lines() {
        let devices = parse_pci_ids(MINI);
        // `\t\t1043 87b3  TUF Gaming` is a (subvendor, subdevice) pair, not a
        // device ID. Read as one it would invent a device 0x1043.
        assert!(
            !devices.iter().any(|d| d.name.contains("TUF Gaming")),
            "subsystem lines must not become devices"
        );
        assert!(!devices.iter().any(|d| d.device_id == 0x1043));
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let devices = parse_pci_ids(MINI);
        assert!(!devices.iter().any(|d| d.name.starts_with('#')));
        assert!(!devices.iter().any(|d| d.name.is_empty()));
    }

    #[test]
    fn device_class_sections_contribute_nothing() {
        // The real file ends with `C xx  <class>` headings whose subclass lines
        // are indented exactly like device lines. A parser that keeps the last
        // good vendor when a heading fails to parse files every one of them
        // under Intel, inventing devices 0x00, 0x01 and 0x03 that no card has.
        let devices = parse_pci_ids(MINI);
        for name in [
            "Non-VGA unclassified device",
            "VGA compatible unclassified device",
            "VGA compatible controller",
        ] {
            assert!(
                !devices.iter().any(|d| d.name == name),
                "class subclass {name:?} leaked into the device list"
            );
        }
        assert_eq!(
            devices.len(),
            5,
            "expected exactly the five real device lines, got {:?}",
            devices.iter().map(|d| &d.name).collect::<Vec<_>>()
        );
    }
}
