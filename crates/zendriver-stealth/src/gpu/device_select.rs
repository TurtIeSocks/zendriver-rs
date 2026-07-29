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
/// The shader model is `5_0` because all three shipped D3D11 tiers are feature
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

/// Spread a seed across the whole `u64` range before it indexes anything.
///
/// The splitmix64 finalizer. Needed because callers use small sequential
/// seeds — a fleet is naturally `Seed(0..n)` — and those map to a tiny slice of
/// any range they are scaled into. Taking `seed % 1_000_000` as a fraction of a
/// cumulative weight sum put every seed below 1000 within 0.001 of zero, so
/// every draw returned the catalogue's first entry.
const fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
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
    ///
    /// **Uniform, which is usually not what you want.** Prefer
    /// [`Self::by_share`] unless you specifically need every catalogued device
    /// to be equally likely. A uniform draw makes a GeForce 210 as probable as
    /// the commonest laptop chip, and fingerprint checks in practice ask
    /// whether a combination is *common* rather than whether it is *correct* —
    /// so a rare device is conspicuous even when every value in it is
    /// internally perfect.
    ///
    /// `None` when no catalogued device is coherent for that platform, which
    /// today means Linux: its tier is device-scoped and has no catalogue.
    #[must_use]
    pub fn from_seed(seed: Seed, platform: Platform) -> Option<Self> {
        let pool = Self::pool(platform);
        if pool.is_empty() {
            return None;
        }
        let index = usize::try_from(mix(seed.0) % pool.len() as u64).ok()?;
        Some(Self(pool[index]))
    }

    /// Draw a device by how common it actually is.
    ///
    /// Same determinism as [`Self::from_seed`], but weighted: a device that
    /// 4% of the corpus population reports is drawn about 4% of the time,
    /// where `from_seed` would treat it the same as a card almost nobody has.
    ///
    /// **Prefer this one.** A detection service sitting in the request path
    /// builds its reference set from the traffic it already sees, so what it
    /// can cheaply check is whether a combination recurs across many unrelated
    /// sessions — not whether it matches some ground truth for that GPU. Rarity
    /// is the signal. Drawing by share lands in the dense part of that
    /// distribution; drawing uniformly is equally coherent and considerably
    /// rarer.
    ///
    /// The weights are marginal probabilities over the *whole* corpus, so they
    /// do not sum to 1 over the catalogue — the categories the catalogue
    /// excludes (iOS, Windows-on-ARM, WARP, VM adapters, unmodelled backends)
    /// hold the rest, and filtering to one platform removes more. This
    /// renormalizes over the pool it is actually drawing from; scanning against
    /// a fixed 1.0 would fall off the end of the cumulative sum and answer the
    /// last entry for most seeds.
    ///
    /// `None` on a platform with no catalogued device, as [`Self::from_seed`].
    #[must_use]
    pub fn by_share(seed: Seed, platform: Platform) -> Option<Self> {
        let pool = Self::pool(platform);
        let total: f64 = pool.iter().map(|e| e.weight).sum();
        if pool.is_empty() || total <= 0.0 {
            return None;
        }
        // A fraction of the pool's own mass. `mix` is what makes a fleet built
        // from `Seed(0..n)` walk the whole distribution rather than crowding
        // into its first entry.
        #[allow(clippy::cast_precision_loss)]
        let fraction = mix(seed.0) as f64 / u64::MAX as f64;
        let target = fraction * total;

        let mut acc = 0.0;
        for entry in &pool {
            acc += entry.weight;
            if acc >= target {
                return Some(Self(entry));
            }
        }
        // Only reachable through floating-point accumulation error.
        pool.last().map(|e| Self(e))
    }

    /// The catalogued device closest to a renderer string the host reported.
    ///
    /// **Pure.** Probing a real host needs a browser, which this crate does not
    /// have; `zendriver::nearest_gpu_device` does the probing and calls this.
    /// Splitting it that way is also what makes the matching testable without
    /// launching Chrome.
    ///
    /// A ladder, most specific first, because "nearest" has to mean something
    /// checkable rather than "some entry":
    ///
    /// 1. the same model *and* device id — the host's exact identity;
    /// 2. the same model — a sibling SKU of the same marketing name;
    /// 3. the same vendor on the same tier, commonest first;
    /// 4. the same tier, commonest first.
    ///
    /// `None` when the host's backend has no catalogue at all, which is the
    /// honest answer for Linux and for SwiftShader: there is no shared tier for
    /// an identity to layer over, and returning a Windows GPU because the
    /// caller asked nicely is exactly the incoherence this design removes.
    #[must_use]
    pub fn nearest_to_renderer(renderer: &str) -> Option<Self> {
        let host = crate::gpu::devices::device_for_renderer(renderer)?;
        let candidates: Vec<&'static CatalogueEntry> =
            CATALOGUE.iter().filter(|e| e.tier == host.tier).collect();
        if candidates.is_empty() {
            return None;
        }

        // The host's own model name and id, as ANGLE spells them.
        let lower = renderer.to_ascii_lowercase();
        let heaviest = |pool: &[&'static CatalogueEntry]| -> Option<Self> {
            pool.iter()
                .copied()
                .max_by(|a, b| a.weight.total_cmp(&b.weight))
                .map(Self)
        };

        // 1 + 2: the host names a device the catalogue knows. Compare against
        // the composed string rather than parsing the host's, so the two can
        // never disagree about what a device is called.
        if let Some(exact) = candidates
            .iter()
            .copied()
            .find(|e| compose_renderer(e).eq_ignore_ascii_case(renderer))
        {
            return Some(Self(exact));
        }
        let same_model: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|e| lower.contains(&e.model.to_ascii_lowercase()))
            .collect();
        if !same_model.is_empty() {
            return heaviest(&same_model);
        }

        // 3: same vendor, same backend.
        let same_vendor: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|e| lower.contains(&e.vendor.to_ascii_lowercase()))
            .collect();
        if !same_vendor.is_empty() {
            return heaviest(&same_vendor);
        }

        // 4: same backend at least.
        heaviest(&candidates)
    }

    /// Every catalogued device, or those matching a case-insensitive substring
    /// of the model, most common first.
    ///
    /// Exists so a caller can *browse* the catalogue rather than having to
    /// already know a model name. [`Self::by_name`] answers one device and
    /// refuses ambiguity; this answers the ambiguity.
    #[must_use]
    pub fn search(query: Option<&str>, platform: Option<Platform>) -> Vec<Self> {
        let needle = query.map(|q| q.trim().to_ascii_lowercase());
        let mut hits: Vec<&'static CatalogueEntry> = CATALOGUE
            .iter()
            .filter(|e| platform.is_none_or(|p| platform_skew(p, e.tier).is_none()))
            .filter(|e| {
                needle.as_ref().is_none_or(|n| {
                    n.is_empty() || e.model.to_ascii_lowercase().contains(n.as_str())
                })
            })
            .collect();
        hits.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(a.model.cmp(b.model)));
        hits.into_iter().map(Self).collect()
    }

    /// PCI device id, or `None` on Metal where Apple silicon exposes none.
    #[must_use]
    pub fn device_id(&self) -> Option<u32> {
        self.0.device_id
    }

    /// Share of the corpus population, in the range `0.0..=1.0`.
    #[must_use]
    pub fn share(&self) -> f64 {
        self.0.weight
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

    /// The four committed captures a catalogue entry can reproduce, read rather
    /// than restated: a hand-copied expectation would pass even if the composer
    /// and the captures drifted apart, which is the only thing this checks.
    const NVIDIA: &str = include_str!("../../data/gpu-tiers/d3d11-fl11-nvidia.json");
    const AMD: &str = include_str!("../../data/gpu-tiers/d3d11-fl11.json");
    const INTEL_GEN9: &str = include_str!("../../data/gpu-tiers/d3d11-fl11-intel-gen9.json");
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
            compose_renderer(&entry(
                "Intel(R) HD Graphics 520",
                "Intel",
                Some(0x1916),
                Tier::D3d11Fl11IntelGen9
            )),
            captured_renderer(INTEL_GEN9)
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
    fn every_catalogued_device_derives_a_coherent_webgpu_adapter() {
        // Third coherence rule: the composed string must resolve its own
        // identity. `adapter_for_renderer` owns the model-to-architecture
        // mapping, so deriving through it rather than storing a generation
        // keeps one source of truth that cannot drift from the catalogue.
        for e in CATALOGUE {
            let renderer = compose_renderer(e);
            let adapter = crate::gpu::devices::adapter_for_renderer(&renderer);
            assert_eq!(
                adapter.vendor,
                e.vendor.to_ascii_lowercase(),
                "{renderer} derived vendor {:?}",
                adapter.vendor
            );
            // An empty architecture is legitimate -- Chrome answers "" for a
            // device it does not classify -- but a *wrong* one reads as a
            // device that does not exist, so only emptiness is tolerated.
            assert!(
                adapter.architecture.is_empty()
                    || adapter
                        .architecture
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{renderer} derived a malformed architecture {:?}",
                adapter.architecture
            );
        }
    }

    #[test]
    fn no_catalogued_device_is_platform_skewed_against_its_own_tier() {
        // Every entry must be reachable from exactly the platform whose
        // backend it belongs to, or it is catalogued and undrawable.
        for e in CATALOGUE {
            let platform = match e.tier {
                Tier::MetalMacos => Platform::MacIntel,
                Tier::D3d11Fl11 | Tier::D3d11Fl11Nvidia | Tier::D3d11Fl11IntelGen9 => {
                    Platform::Win32
                }
                other => panic!("{} is on {other:?}, which has no catalogue", e.model),
            };
            assert!(
                platform_skew(platform, e.tier).is_none(),
                "{} claims {platform:?} over {:?}",
                e.model,
                e.tier
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
                    matches!(
                        d.tier(),
                        Tier::D3d11Fl11 | Tier::D3d11Fl11Nvidia | Tier::D3d11Fl11IntelGen9
                    ),
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
    fn a_share_draw_is_deterministic_per_seed() {
        for seed in [0u64, 3, 77, 1_234_567] {
            let a = GpuDevice::by_share(Seed(seed), Platform::Win32);
            let b = GpuDevice::by_share(Seed(seed), Platform::Win32);
            assert_eq!(a.map(|d| d.renderer()), b.map(|d| d.renderer()));
        }
    }

    #[test]
    fn every_catalogued_entry_carries_a_usable_weight() {
        // A zero-weight row can never be drawn by share, so it would be
        // invisible to fleet diversity while still selectable by name.
        for e in CATALOGUE {
            assert!(
                e.weight > 0.0,
                "{} ({:?}) has no share weight",
                e.model,
                e.device_id
            );
        }
    }

    #[test]
    fn a_share_draw_spreads_and_never_crosses_platforms() {
        // The pool's mass is well under 1, so a scan against a fixed 1.0 would
        // fall off the end and answer the last entry for most seeds. That bug
        // passes a determinism test, so this one checks coverage instead.
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..1_000u64 {
            let d = GpuDevice::by_share(Seed(seed), Platform::Win32).unwrap();
            assert!(
                matches!(
                    d.tier(),
                    Tier::D3d11Fl11 | Tier::D3d11Fl11Nvidia | Tier::D3d11Fl11IntelGen9
                ),
                "seed {seed} drew {:?} for Win32",
                d.tier()
            );
            seen.insert(d.renderer());
        }
        assert!(
            seen.len() > 20,
            "share draw covered only {} devices",
            seen.len()
        );
    }

    #[test]
    fn a_share_draw_favours_the_common_devices() {
        // The point of weighting: the heaviest entry must come up far more
        // often than a uniform draw would give it.
        let pool: Vec<_> = CATALOGUE
            .iter()
            .filter(|e| platform_skew(Platform::MacIntel, e.tier).is_none())
            .collect();
        let heaviest = pool
            .iter()
            .max_by(|a, b| a.weight.total_cmp(&b.weight))
            .unwrap();
        let draws = 2_000u64;
        let hits = (0..draws)
            .filter_map(|s| GpuDevice::by_share(Seed(s), Platform::MacIntel))
            .filter(|d| d.model() == heaviest.model)
            .count();
        let uniform = draws as usize / pool.len();
        assert!(
            hits > uniform,
            "weighting did nothing: {hits} hits vs {uniform} for a uniform draw"
        );
    }

    #[test]
    fn nearest_matches_the_hosts_own_identity_exactly_when_it_can() {
        let rtx = GpuDevice::by_name("NVIDIA GeForce RTX 4090").unwrap();
        let got = GpuDevice::nearest_to_renderer(&rtx.renderer()).unwrap();
        assert_eq!(got.renderer(), rtx.renderer());
    }

    #[test]
    fn nearest_falls_back_through_model_then_vendor_then_tier() {
        // A model the catalogue knows, but a device id it has never seen
        // paired with that name: rung 2, same model, different SKU.
        let sibling = "ANGLE (NVIDIA, NVIDIA GeForce RTX 4090 (0x0000DEAD)                        Direct3D11 vs_5_0 ps_5_0, D3D11)";
        let got = GpuDevice::nearest_to_renderer(sibling).unwrap();
        assert_eq!(got.model(), "NVIDIA GeForce RTX 4090");

        // A vendor it knows, a model it does not: rung 3.
        let unknown_nvidia = "ANGLE (NVIDIA, NVIDIA GeForce RTX 9090 Ti                               (0x0000BEEF) Direct3D11 vs_5_0 ps_5_0, D3D11)";
        let got = GpuDevice::nearest_to_renderer(unknown_nvidia).unwrap();
        assert_eq!(got.vendor(), "NVIDIA");
        assert_eq!(got.tier(), Tier::D3d11Fl11Nvidia);
    }

    #[test]
    fn nearest_answers_none_where_no_catalogue_exists() {
        // Both are real backends with no shared tier to layer an identity
        // over. Answering a Windows GPU because the caller asked would be the
        // exact incoherence the catalogue exists to remove.
        for renderer in [
            "ANGLE (Intel, Vulkan 1.4.318 (Intel(R) Iris(R) Pro Graphics 580              (SKL GT4) (0x0000193B)), Intel open-source Mesa driver)",
            "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero)              (0x0000C0DE)), SwiftShader driver)",
        ] {
            assert!(
                GpuDevice::nearest_to_renderer(renderer).is_none(),
                "{renderer} must have no nearest catalogued device"
            );
        }
        // A renderer no tier covers at all.
        assert!(GpuDevice::nearest_to_renderer("Mali-G715").is_none());
    }

    #[test]
    fn search_browses_where_by_name_refuses() {
        // by_name answers one device and errors on ambiguity; search is how a
        // caller discovers what the ambiguous names actually are.
        assert!(matches!(
            GpuDevice::by_name("rtx 40"),
            Err(DeviceLookupError::Ambiguous(_))
        ));
        let hits = GpuDevice::search(Some("rtx 40"), Some(Platform::Win32));
        assert!(hits.len() > 1, "search found {}", hits.len());
        assert!(hits.iter().all(|d| d.model().contains("RTX 40")));
        // Commonest first, so a caller taking the head gets a sensible default.
        assert!(hits[0].share() >= hits[hits.len() - 1].share());
    }

    #[test]
    fn search_with_no_query_lists_the_platforms_devices() {
        let win = GpuDevice::search(None, Some(Platform::Win32));
        let mac = GpuDevice::search(None, Some(Platform::MacIntel));
        let all = GpuDevice::search(None, None);
        assert_eq!(all.len(), CATALOGUE.len());
        assert_eq!(
            win.len() + mac.len(),
            all.len(),
            "every entry is one or the other"
        );
        assert!(mac.iter().all(|d| d.tier() == Tier::MetalMacos));
        assert!(win.iter().all(|d| d.device_id().is_some()));
    }

    #[test]
    fn linux_has_no_catalogue_because_its_tier_is_device_scoped() {
        // ANGLE's Vulkan backend reads caps off the physical device, so there
        // is no shared tier for a Linux identity to layer over.
        assert!(GpuDevice::from_seed(Seed(1), Platform::LinuxX86_64).is_none());
    }
}
