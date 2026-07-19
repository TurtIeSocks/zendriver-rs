//! Generative source (C3): a browserforge/apify Bayesian-network persona generator.
//!
//! Ports the canonical conditional-probability-table form: each node's
//! distribution is conditioned on its parents' sampled values, walked through a
//! nested `conditionalProbabilities` `deeper`/`skip` tree. Sampling is
//! **deterministic** in the [`Seed`] (a seeded Mulberry32 stream) and yields an
//! internally **coherent**, **desktop** [`Persona`]: the single root `userAgent`
//! node is restricted to non-mobile UAs, and every other attribute is sampled
//! conditioned on it.
//!
//! The network is fetched on first use and cached (see [`Generator::load_or_download`]).
//! Determinism holds for a fixed cached network version; a cache refresh (the
//! upstream file is regenerated ~monthly) may change per-seed output.

mod download;
mod mapping;

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use zendriver_stealth::{Persona, Seed};

/// Upstream fingerprint network (apify/fingerprint-suite, Apache-2.0, regenerated
/// ~monthly). Overridable per call via [`Generator::load_or_download`] or the
/// `ZENDRIVER_FP_NETWORK_URL` env var (see [`Generator::load_or_download_default`]).
pub const DEFAULT_NETWORK_URL: &str = "https://raw.githubusercontent.com/apify/fingerprint-suite/master/packages/fingerprint-generator/src/data_files/fingerprint-network-definition.zip";

/// Test-support: bytes of the tiny embedded network zip, for downstream hermetic
/// tests (e.g. `zendriver-mcp`). Not part of the supported API.
#[doc(hidden)]
pub const TEST_NETWORK_ZIP: &[u8] = include_bytes!("fixtures/mini-network.zip");

/// Errors from loading or decompressing the network.
#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("empty network archive")]
    EmptyArchive,
}

#[derive(Debug, Clone, Deserialize)]
struct RawNetwork {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone, Deserialize)]
struct Node {
    name: String,
    #[serde(default, rename = "parentNames")]
    parent_names: Vec<String>,
    /// Canonical nested CPT, walked dynamically by depth = `parent_names.len()`.
    #[serde(rename = "conditionalProbabilities")]
    cpt: Value,
}

/// A Bayesian network of conditional fingerprint attributes (browserforge form).
#[derive(Debug, Clone)]
pub struct Generator {
    nodes: Arc<Vec<Node>>,
}

impl Generator {
    /// Build from the inner network JSON (seam for fast unit tests).
    pub fn from_network_json(json: &str) -> Result<Self, GenError> {
        let raw: RawNetwork = serde_json::from_str(json)?;
        Ok(Self {
            nodes: Arc::new(raw.nodes),
        })
    }

    /// Build from raw ZIP bytes (decompress the first entry, then parse).
    pub fn from_zip_bytes(bytes: &[u8]) -> Result<Self, GenError> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        if archive.is_empty() {
            return Err(GenError::EmptyArchive);
        }
        let mut entry = archive.by_index(0)?;
        let mut json = String::new();
        entry.read_to_string(&mut json)?;
        Self::from_network_json(&json)
    }

    /// Fetch (or read cached) the network ZIP from `url`, then build.
    ///
    /// `policy` controls cache freshness — see [`CachePolicy`](crate::CachePolicy).
    /// Pass `CachePolicy::default()` for the original permanent-cache behavior.
    ///
    /// ```no_run
    /// use zendriver_fingerprints::CachePolicy;
    /// use zendriver_fingerprints::generative::Generator;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // Always re-download, ignoring any cached network.
    /// let generator = Generator::load_or_download(
    ///     "https://example.com/network.zip",
    ///     CachePolicy::force_refresh(),
    /// )
    /// .await?;
    /// # let _ = generator;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn load_or_download(url: &str, policy: crate::CachePolicy) -> Result<Self, GenError> {
        let cache = download::cache_path();
        let bytes = download::fetch_or_cached_bytes(url, &cache, policy).await?;
        Self::from_zip_bytes(&bytes)
    }

    /// Ergonomic [`load_or_download`](Self::load_or_download): uses
    /// `ZENDRIVER_FP_NETWORK_URL` if set, else [`DEFAULT_NETWORK_URL`].
    pub async fn load_or_download_default(policy: crate::CachePolicy) -> Result<Self, GenError> {
        let url = std::env::var("ZENDRIVER_FP_NETWORK_URL")
            .unwrap_or_else(|_| DEFAULT_NETWORK_URL.to_string());
        Self::load_or_download(&url, policy).await
    }

    /// Sample a coherent **desktop** persona deterministically from `seed`.
    pub fn generate(&self, seed: Seed) -> Persona {
        let mut rng = Mulberry32::new(seed.value() as u32);
        let mut assigned: HashMap<String, String> = HashMap::new();
        for node in self.topo_order() {
            let leaf = walk_cpt(&node.cpt, &node.parent_names, &assigned);
            // Restrict the single root `userAgent` node to desktop UAs so the
            // whole persona is desktop-coherent without backtracking.
            let dist = leaf_distribution(leaf, node.name == "userAgent");
            if dist.is_empty() {
                continue;
            }
            assigned.insert(node.name.clone(), weighted_pick(&dist, rng.next_f64()));
        }
        mapping::persona_from_assignment(&assigned)
    }

    /// Generate a persona, then overlay a coherent locale/languages for
    /// `country`. The geo overlay wins over the (locale-free) generated base.
    #[cfg(feature = "geo")]
    pub fn generate_geo(&self, seed: Seed, country: zendriver_stealth::geo::Country) -> Persona {
        self.generate(seed)
            .overlay(zendriver_stealth::geo::persona(country))
    }

    /// Nodes in parents-before-children order (stable sort by parent count;
    /// ties keep JSON array order — keeps [`generate`](Self::generate)
    /// deterministic).
    fn topo_order(&self) -> Vec<&Node> {
        let mut order: Vec<&Node> = self.nodes.iter().collect();
        order.sort_by_key(|n| n.parent_names.len());
        order
    }
}

/// Walk the nested CPT to the leaf for the parents' sampled values. For each
/// parent, descend `deeper[value]` if present, else `skip`. After
/// `parents.len()` steps the result is the leaf `{ value: probability }` object
/// (or `Null` for a missing/`null` `skip`).
fn walk_cpt<'a>(
    cpt: &'a Value,
    parents: &[String],
    assigned: &HashMap<String, String>,
) -> &'a Value {
    const NULL: Value = Value::Null;
    let mut cur = cpt;
    for parent in parents {
        let pv = assigned.get(parent).map(String::as_str).unwrap_or_default();
        // Descend `deeper[parent_value]` when present, else the `skip` branch
        // (absent / `null` -> empty leaf). Plain match avoids any borrow ambiguity.
        cur = match cur.get("deeper").and_then(|d| d.get(pv)) {
            Some(next) => next,
            None => cur.get("skip").unwrap_or(&NULL),
        };
    }
    cur
}

/// Flatten a leaf object into a `(value, weight)` distribution. When
/// `desktop_only`, keep only desktop-UA keys (used for the root node).
fn leaf_distribution(leaf: &Value, desktop_only: bool) -> Vec<(String, f64)> {
    let Some(obj) = leaf.as_object() else {
        return Vec::new();
    };
    obj.iter()
        .filter(|(k, _)| !desktop_only || mapping::is_desktop_ua(k))
        .filter_map(|(k, v)| v.as_f64().map(|w| (k.clone(), w)))
        .collect()
}

/// Weighted pick from a (value, weight) distribution given a uniform `r` in
/// `[0, 1)`. Weights need not be normalized.
fn weighted_pick(dist: &[(String, f64)], r: f64) -> String {
    let total: f64 = dist.iter().map(|(_, w)| w).sum();
    if total <= 0.0 {
        return dist.first().map(|(v, _)| v.clone()).unwrap_or_default();
    }
    let mut acc = 0.0;
    for (v, w) in dist {
        acc += w / total;
        if r <= acc {
            return v.clone();
        }
    }
    dist.last().map(|(v, _)| v.clone()).unwrap_or_default()
}

/// Mulberry32 PRNG — tiny, fast, deterministic.
struct Mulberry32(u32);

impl Mulberry32 {
    fn new(seed: u32) -> Self {
        Self(seed)
    }

    /// Next value in `[0, 1)`.
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6D2B_79F5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(1 | t);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use zendriver_stealth::Platform;

    const FIXTURE: &str = include_str!("fixtures/mini-network.json");

    fn make_gen() -> Generator {
        Generator::from_network_json(FIXTURE).expect("fixture parses")
    }

    #[test]
    fn parses_fixture_with_root_user_agent() {
        let g = make_gen();
        assert_eq!(g.nodes.len(), 6);
        let roots: Vec<_> = g
            .nodes
            .iter()
            .filter(|n| n.parent_names.is_empty())
            .collect();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].name, "userAgent");
    }

    #[test]
    fn generate_is_deterministic() {
        let g = make_gen();
        for s in 0..64u64 {
            let a = g.generate(Seed::from_u64(s));
            let b = g.generate(Seed::from_u64(s));
            assert_eq!(
                serde_json::to_value(&a).unwrap(),
                serde_json::to_value(&b).unwrap()
            );
        }
    }

    #[test]
    fn personas_are_desktop_coherent() {
        let g = make_gen();
        for s in 0..256u64 {
            let p = g.generate(Seed::from_u64(s));
            let plat = p.platform.expect("platform assigned");
            assert!(
                matches!(
                    plat,
                    Platform::Win32 | Platform::MacIntel | Platform::LinuxX86_64
                ),
                "non-desktop platform sampled: {plat:?}"
            );
            let w = p.webgl.as_ref().expect("webgl assigned");
            let renderer = w.unmasked_renderer.as_deref().expect("renderer assigned");
            assert!(w.unmasked_vendor.is_some(), "vendor assigned");
            match plat {
                Platform::Win32 => assert!(renderer.contains("D3D11"), "win renderer: {renderer}"),
                Platform::MacIntel => {
                    assert!(
                        renderer.contains("Apple") || renderer.contains("Metal"),
                        "mac renderer: {renderer}"
                    )
                }
                Platform::LinuxX86_64 => {
                    assert!(renderer.contains("Mesa"), "linux renderer: {renderer}")
                }
            }
        }
    }

    #[test]
    fn fields_populated() {
        let g = make_gen();
        let p = g.generate(Seed::from_u64(1));
        assert!(p.device_memory_gb.is_some());
        assert!(p.hardware_concurrency.is_some());
        assert!(
            p.fonts
                .as_ref()
                .and_then(|f| f.available.as_ref())
                .is_some()
        );
    }

    #[test]
    fn different_seeds_can_differ() {
        let g = make_gen();
        let plats: Vec<_> = (0..64u64)
            .map(|s| g.generate(Seed::from_u64(s)).platform)
            .collect();
        let first = plats[0];
        assert!(
            plats.iter().any(|p| *p != first),
            "every seed gave same platform"
        );
    }

    #[test]
    fn from_zip_bytes_loads_fixture() {
        let g = Generator::from_zip_bytes(TEST_NETWORK_ZIP).expect("zip loads");
        assert!(g.generate(Seed::from_u64(3)).platform.is_some());
    }

    #[cfg(feature = "geo")]
    #[test]
    fn generate_geo_overlays_locale() {
        let generator = Generator::from_network_json(include_str!("fixtures/mini-network.json"))
            .expect("load mini network");
        let country = zendriver_stealth::geo::Country::try_from("DE").unwrap();
        let p = generator.generate_geo(Seed::from_u64(1), country);
        assert_eq!(p.locale.as_deref(), Some("de-DE"));
        assert_eq!(p.languages.unwrap(), vec!["de-DE", "de"]);
    }

    #[tokio::test]
    async fn load_or_download_builds_from_mock() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(TEST_NETWORK_ZIP))
            .mount(&server)
            .await;
        let g = Generator::load_or_download(&server.uri(), crate::CachePolicy::default())
            .await
            .expect("load");
        assert!(g.generate(Seed::from_u64(5)).platform.is_some());
    }
}
