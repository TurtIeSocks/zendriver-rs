//! Choosing a catalogued GPU identity, and composing its renderer string.
//!
//! The catalogue widens *which device* a persona can claim. It never widens
//! what a device can do: capability values still come from the measured tier
//! the entry names, so nothing here can invent one.

use crate::Platform;
use crate::gpu::catalogue::CATALOGUE;
use crate::gpu::invariants::platform_skew;
use crate::gpu::types::{CatalogueEntry, Tier};
use crate::persona::Seed;

/// Build the renderer string ANGLE reports for a device.
///
/// Composed rather than stored, and this is the only implementation. Storing
/// several hundred strings would turn a Chrome format change into several
/// hundred edits, and that format has already changed once: the fingerprint
/// corpus contains strings from before ANGLE appended the device id it now
/// always includes.
///
/// D3D11, `Renderer11.cpp:2308-2319`:
///
/// ```text
/// mDescription << " (" << FmtHex(DeviceId) << ")" << " Direct3D11"
///              << " vs_" << major << "_" << minor << " ps_" << ...
/// ```
///
/// The shader model is `5_0` because both shipped D3D11 tiers are feature
/// level 11 and the catalogue excludes anything lower — ANGLE writes the
/// feature level into this field, so an FL10 part would report `vs_4_1` here
/// and is dropped at generation time rather than misrepresented.
///
/// Metal, `DisplayMtl.mm:188-201`, has one variable and no device id: Apple
/// silicon exposes no PCI id. Its trailing field is the literal
/// `getVersionString` returns for WebGL contexts (`:216`), not a version.
#[must_use]
pub(crate) fn compose_renderer(entry: &CatalogueEntry) -> String {
    match entry.device_id {
        Some(id) => format!(
            "ANGLE ({}, {} (0x{id:08X}) Direct3D11 vs_5_0 ps_5_0, D3D11)",
            entry.vendor, entry.model
        ),
        None => format!(
            "ANGLE ({}, ANGLE Metal Renderer: {}, Unspecified Version)",
            entry.vendor, entry.model
        ),
    }
}

/// A catalogued GPU identity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuDevice(&'static CatalogueEntry);

/// Why a device lookup did not produce exactly one answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceLookupError {
    /// No catalogued model contains the query.
    NotFound(String),
    /// Several distinct models contain the query. Answering with one of them
    /// would silently pick a device the caller did not ask for, so the
    /// candidates are returned instead.
    Ambiguous(Vec<&'static str>),
}

impl std::fmt::Display for DeviceLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(q) => write!(f, "no catalogued GPU matches {q:?}"),
            Self::Ambiguous(c) => {
                write!(f, "{} catalogued GPUs match; name one of: ", c.len())?;
                write!(f, "{}", c.join(", "))
            }
        }
    }
}

impl std::error::Error for DeviceLookupError {}

impl GpuDevice {
    /// Driver-reported model text, e.g. `NVIDIA GeForce RTX 4090`.
    #[must_use]
    pub fn model(&self) -> &'static str {
        self.0.model
    }

    /// ANGLE's vendor token, e.g. `NVIDIA`.
    #[must_use]
    pub fn vendor(&self) -> &'static str {
        self.0.vendor
    }

    /// The renderer string a page reads for this device.
    #[must_use]
    pub fn renderer(&self) -> String {
        compose_renderer(self.0)
    }

    /// Which measured tier supplies this device's capability values.
    ///
    /// Read by the coherence tests here and by `Persona` wiring in the next
    /// task; not part of the public surface, since a caller picks a device
    /// rather than a tier.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn tier(&self) -> Tier {
        self.0.tier
    }

    /// Look a device up by name, case-insensitively, on any substring of the
    /// model.
    ///
    /// Refuses rather than guesses. `"rtx 30"` matches the whole Ampere line,
    /// and answering with one of them would hand back a device the caller did
    /// not ask for — so every candidate is returned and the caller narrows it.
    ///
    /// An exact model name always wins, which is what makes the narrowing
    /// possible: several catalogued names are prefixes of others, so
    /// `"NVIDIA GeForce RTX 4090"` would otherwise be permanently ambiguous
    /// against `RTX 4090 D` and `RTX 4090 Laptop GPU` and could never be
    /// selected at all.
    ///
    /// Where one model spans several SKUs (the corpus reports 77 such names,
    /// because a marketing name really does cover several parts), the most
    /// common pairing wins. Every candidate is a device that reports that
    /// name; the heaviest is the one most machines would.
    ///
    /// # Errors
    /// [`DeviceLookupError::NotFound`] or [`DeviceLookupError::Ambiguous`].
    pub fn by_name(query: &str) -> Result<Self, DeviceLookupError> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Err(DeviceLookupError::NotFound(query.to_string()));
        }
        // An exact name wins outright. Without this the plain
        // "NVIDIA GeForce RTX 4090" is unselectable, because it is a prefix of
        // "RTX 4090 D" and "RTX 4090 Laptop GPU" and would always come back
        // ambiguous against its own longer siblings.
        let exact: Vec<&'static CatalogueEntry> = CATALOGUE
            .iter()
            .filter(|e| e.model.to_ascii_lowercase() == needle)
            .collect();
        let hits: Vec<&'static CatalogueEntry> = if exact.is_empty() {
            CATALOGUE
                .iter()
                .filter(|e| e.model.to_ascii_lowercase().contains(&needle))
                .collect()
        } else {
            exact
        };

        let mut names: Vec<&'static str> = hits.iter().map(|e| e.model).collect();
        names.sort_unstable();
        names.dedup();
        match names.len() {
            0 => Err(DeviceLookupError::NotFound(query.to_string())),
            1 => Ok(Self(
                hits.iter()
                    .copied()
                    .max_by(|a, b| a.weight.total_cmp(&b.weight))
                    .expect("a name matched, so there is at least one entry"),
            )),
            _ => Err(DeviceLookupError::Ambiguous(names)),
        }
    }

    /// Draw a device deterministically from a seed.
    ///
    /// The same seed always yields the same device, so a persona's GPU is
    /// reproducible alongside the seeded farbling `Persona` already does.
    /// Uniform over the platform's entries; [`Self::by_share`] draws by how
    /// common each one actually is.
    ///
    /// `None` when no catalogued device is coherent for that platform, which
    /// today means Linux: its tier is device-scoped and has no catalogue.
    #[must_use]
    pub fn from_seed(seed: Seed, platform: Platform) -> Option<Self> {
        let pool = Self::pool(platform);
        if pool.is_empty() {
            return None;
        }
        let index = usize::try_from(seed.0 % pool.len() as u64).ok()?;
        Some(Self(pool[index]))
    }

    /// Every catalogue entry coherent with `platform`.
    ///
    /// Filtered through the same `platform_skew` check the invariants use, so
    /// a device can never be drawn for an OS its backend does not exist on.
    fn pool(platform: Platform) -> Vec<&'static CatalogueEntry> {
        CATALOGUE
            .iter()
            .filter(|e| platform_skew(platform, e.tier).is_none())
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The three committed captures, read rather than restated: a hand-copied
    /// expectation would pass even if the composer and the captures drifted
    /// apart, which is the only thing this checks.
    const NVIDIA: &str = include_str!("../../data/gpu-tiers/d3d11-fl11-nvidia.json");
    const AMD: &str = include_str!("../../data/gpu-tiers/d3d11-fl11.json");
    const METAL: &str = include_str!("../../data/gpu-tiers/metal-macos.json");

    fn captured_renderer(raw: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        v["capture"]["webgl2"]["unmaskedRenderer"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn entry(model: &str, vendor: &str, device_id: Option<u32>, tier: Tier) -> CatalogueEntry {
        CatalogueEntry {
            model: Box::leak(model.to_string().into_boxed_str()),
            vendor: Box::leak(vendor.to_string().into_boxed_str()),
            device_id,
            tier,
            weight: 0.0,
        }
    }

    #[test]
    fn composition_reproduces_every_committed_capture() {
        assert_eq!(
            compose_renderer(&entry(
                "NVIDIA GeForce RTX 4090",
                "NVIDIA",
                Some(0x2684),
                Tier::D3d11Fl11Nvidia
            )),
            captured_renderer(NVIDIA)
        );
        assert_eq!(
            compose_renderer(&entry(
                "AMD Radeon(TM) Graphics",
                "AMD",
                Some(0x164E),
                Tier::D3d11Fl11
            )),
            captured_renderer(AMD)
        );
        assert_eq!(
            compose_renderer(&entry("Apple M4 Pro", "Apple", None, Tier::MetalMacos)),
            captured_renderer(METAL)
        );
    }

    #[test]
    fn every_catalogued_device_round_trips_to_its_own_tier() {
        // The catalogue may widen identity, never capability. An entry whose
        // composed string resolves elsewhere would serve one device's name
        // over another's numbers.
        for e in CATALOGUE {
            let renderer = compose_renderer(e);
            assert_eq!(
                crate::gpu::devices::device_for_renderer(&renderer).map(|d| d.tier),
                Some(e.tier),
                "{renderer} resolved the wrong tier"
            );
        }
    }

    #[test]
    fn every_catalogued_device_reports_its_own_vendor() {
        for e in CATALOGUE {
            let renderer = compose_renderer(e);
            let derived = crate::gpu::devices::vendor_for_renderer(&renderer);
            assert_eq!(
                derived.as_deref(),
                Some(format!("Google Inc. ({})", e.vendor).as_str()),
                "{renderer} derived the wrong vendor"
            );
        }
    }

    #[test]
    fn by_name_finds_a_device_case_insensitively() {
        let d = GpuDevice::by_name("nvidia geforce rtx 4090").unwrap();
        assert_eq!(d.model(), "NVIDIA GeForce RTX 4090");
        assert_eq!(d.tier(), Tier::D3d11Fl11Nvidia);
    }

    #[test]
    fn an_exact_name_beats_the_longer_names_containing_it() {
        // The catalogue holds "RTX 4090", "RTX 4090 D" and "RTX 4090 Laptop
        // GPU". Substring matching alone makes the first unselectable by its
        // own full name, which is the one name a caller is most likely to use.
        let d = GpuDevice::by_name("NVIDIA GeForce RTX 4090").unwrap();
        assert_eq!(d.model(), "NVIDIA GeForce RTX 4090");
        // The partial query stays ambiguous, as it should.
        assert!(matches!(
            GpuDevice::by_name("rtx 4090"),
            Err(DeviceLookupError::Ambiguous(_))
        ));
    }

    #[test]
    fn by_name_refuses_an_ambiguous_query_rather_than_guessing() {
        match GpuDevice::by_name("rtx 40") {
            Err(DeviceLookupError::Ambiguous(c)) => {
                assert!(c.len() > 1, "expected several candidates, got {c:?}");
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn by_name_reports_a_miss_rather_than_answering_something_else() {
        assert!(matches!(
            GpuDevice::by_name("definitely not a gpu"),
            Err(DeviceLookupError::NotFound(_))
        ));
        assert!(matches!(
            GpuDevice::by_name("   "),
            Err(DeviceLookupError::NotFound(_))
        ));
    }

    #[test]
    fn one_name_spanning_several_skus_resolves_to_the_commonest() {
        // 77 catalogued names cover more than one device id. Any of them is a
        // real pairing, so the tie-break is which one most machines report.
        let d = GpuDevice::by_name("Intel(R) Iris(R) Xe Graphics").unwrap();
        let all: Vec<_> = CATALOGUE.iter().filter(|e| e.model == d.model()).collect();
        assert!(all.len() > 1, "expected a multi-SKU name");
        let heaviest = all
            .iter()
            .max_by(|a, b| a.weight.total_cmp(&b.weight))
            .unwrap();
        assert_eq!(d.renderer(), compose_renderer(heaviest));
    }

    #[test]
    fn the_same_seed_always_draws_the_same_device() {
        for seed in [0u64, 1, 42, 9_999] {
            let a = GpuDevice::from_seed(Seed(seed), Platform::Win32);
            let b = GpuDevice::from_seed(Seed(seed), Platform::Win32);
            assert_eq!(a.map(|d| d.model()), b.map(|d| d.model()));
        }
    }

    #[test]
    fn a_seeded_draw_never_crosses_platforms() {
        for seed in 0..200u64 {
            if let Some(d) = GpuDevice::from_seed(Seed(seed), Platform::MacIntel) {
                assert_eq!(d.tier(), Tier::MetalMacos, "seed {seed} crossed platforms");
            }
            if let Some(d) = GpuDevice::from_seed(Seed(seed), Platform::Win32) {
                assert!(
                    matches!(d.tier(), Tier::D3d11Fl11 | Tier::D3d11Fl11Nvidia),
                    "seed {seed} drew {:?} for Win32",
                    d.tier()
                );
            }
        }
    }

    #[test]
    fn seeded_draws_actually_spread_across_the_pool() {
        // A modulo bug that always hit index 0 would satisfy determinism and
        // the platform filter while making the catalogue pointless.
        let seen: std::collections::BTreeSet<_> = (0..300u64)
            .filter_map(|s| GpuDevice::from_seed(Seed(s), Platform::Win32))
            .map(|d| d.renderer())
            .collect();
        assert!(
            seen.len() > 50,
            "seeded draw covered only {} devices",
            seen.len()
        );
    }

    #[test]
    fn linux_has_no_catalogue_because_its_tier_is_device_scoped() {
        // ANGLE's Vulkan backend reads caps off the physical device, so there
        // is no shared tier for a Linux identity to layer over.
        assert!(GpuDevice::from_seed(Seed(1), Platform::LinuxX86_64).is_none());
    }
}
