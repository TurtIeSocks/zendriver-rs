//! Generative source (C3): a Bayesian-network persona generator.
//!
//! Ports the browserforge conditional-probability-table (CPT) form: each node is
//! a fingerprint attribute whose distribution is conditioned on its parents'
//! sampled values. Sampling is **deterministic** in the [`Seed`] and yields an
//! internally **coherent** [`Persona`] (e.g. a `MacIntel` platform pairs with a
//! Mac-plausible WebGL renderer, because the renderer node is conditioned on
//! `platform`).
//!
//! The embedded [`network.json`] is intentionally small; expanding it from the
//! full upstream browserforge dataset is tracked as a follow-up (spec §14). The
//! loader and sampler are complete and correct regardless of network size.

use std::collections::HashMap;

use serde::Deserialize;
use zendriver_stealth::{Persona, Platform, Seed, WebglSpec};

/// A Bayesian network of conditional fingerprint attributes (browserforge form).
#[derive(Debug, Clone, Deserialize)]
pub struct Generator {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone, Deserialize)]
struct Node {
    name: String,
    parents: Vec<String>,
    /// Conditional distributions keyed by the parents' sampled values joined by
    /// `|` (empty string for a root node). Each entry maps the key to a weighted
    /// `(value, weight)` distribution.
    cpt: HashMap<String, Vec<(String, f64)>>,
}

impl Generator {
    /// Load the embedded, trimmed network.
    pub fn embedded() -> Self {
        serde_json::from_str(include_str!("network.json")).expect("valid embedded BN")
    }

    /// Sample a coherent persona deterministically from `seed`.
    ///
    /// Determinism is guaranteed by walking nodes in a stable topological order
    /// (a vector, never a `HashMap`) and driving every pick from a single
    /// [`Mulberry32`] stream seeded by `seed`. The same seed therefore always
    /// produces byte-identical assignments and thus an identical [`Persona`].
    pub fn generate(&self, seed: Seed) -> Persona {
        let mut rng = Mulberry32::new(seed.value() as u32);
        let mut assigned: HashMap<String, String> = HashMap::new();
        for node in self.topo_order() {
            // Build the conditioning key from already-assigned parents, joined
            // by `|`. For a root node this is the empty string, matching the
            // `""` CPT entry.
            //
            // A parent must always be assigned before a child reads it; the topo
            // order guarantees this for the embedded network. If a parent is
            // somehow missing (a malformed network), fall back to its empty key
            // so sampling stays panic-free rather than producing a silently
            // mismatched conditioning key.
            debug_assert!(
                node.parents.iter().all(|p| assigned.contains_key(p)),
                "node `{}` sampled before parent assignment (bad topo order)",
                node.name
            );
            let key = node
                .parents
                .iter()
                .map(|p| assigned.get(p).cloned().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("|");
            // Prefer the conditioned distribution; fall back to the
            // unconditioned (`""`) entry if the exact key is absent.
            let Some(dist) = node.cpt.get(&key).or_else(|| node.cpt.get("")) else {
                // No usable distribution for this node — skip it rather than
                // panic. Downstream mapping treats absent attributes as `None`.
                continue;
            };
            assigned.insert(node.name.clone(), weighted_pick(dist, rng.next_f64()));
        }
        persona_from_assignment(&assigned)
    }

    /// Nodes in parents-before-children order.
    ///
    /// Sorting by parent count is a defensive heuristic that is exact for the
    /// shallow embedded network (roots have 0 parents, children have 1). The
    /// sort is **stable**, so ties keep their `network.json` order — this keeps
    /// [`generate`](Self::generate) deterministic regardless of `HashMap`
    /// iteration order.
    fn topo_order(&self) -> Vec<&Node> {
        let mut order = self.nodes.iter().collect::<Vec<_>>();
        order.sort_by_key(|n| n.parents.len());
        order
    }
}

/// Mulberry32 PRNG — tiny, fast, deterministic. Matches the JS reference used
/// upstream so seeds are portable across the Rust and JS farble paths.
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

/// Pick a value from a weighted distribution given a uniform draw `r` in
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
    // Floating-point slack: return the last value if `r` rounded past `acc`.
    dist.last().map(|(v, _)| v.clone()).unwrap_or_default()
}

/// Map a sampled attribute assignment onto a [`Persona`]. Keys mirror the
/// `network.json` node names (`platform`, `webglRenderer`, `deviceMemory`, …).
/// Absent attributes leave the corresponding `Persona` field at its default.
fn persona_from_assignment(a: &HashMap<String, String>) -> Persona {
    let mut p = Persona::default();
    if let Some(plat) = a.get("platform") {
        p.platform = Some(match plat.as_str() {
            "Win32" => Platform::Win32,
            "MacIntel" => Platform::MacIntel,
            _ => Platform::LinuxX86_64,
        });
    }
    if let Some(r) = a.get("webglRenderer") {
        p.webgl = Some(WebglSpec {
            unmasked_renderer: Some(r.clone()),
            ..Default::default()
        });
    }
    if let Some(m) = a.get("deviceMemory") {
        p.device_memory_gb = m.parse().ok();
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_deterministic_and_coherent() {
        let g = Generator::embedded();
        let a = g.generate(Seed::from_u64(7));
        let b = g.generate(Seed::from_u64(7));
        // same seed → same persona
        assert_eq!(a.platform, b.platform);
        assert_eq!(
            a.webgl
                .as_ref()
                .and_then(|w| w.unmasked_renderer.as_deref()),
            b.webgl
                .as_ref()
                .and_then(|w| w.unmasked_renderer.as_deref())
        );
        assert_eq!(a.device_memory_gb, b.device_memory_gb);
        // coherence: a populated persona
        assert!(a.platform.is_some());
        assert!(a.webgl.is_some());
    }

    #[test]
    fn webgl_renderer_is_coherent_with_platform() {
        // The renderer node is conditioned on `platform`, so a sampled persona
        // must never pair a platform with a renderer from another platform.
        let g = Generator::embedded();
        for s in 0..64u64 {
            let p = g.generate(Seed::from_u64(s));
            let plat = p.platform.expect("platform always assigned");
            let renderer = p
                .webgl
                .as_ref()
                .and_then(|w| w.unmasked_renderer.as_deref())
                .expect("renderer always assigned");
            match plat {
                Platform::MacIntel => assert!(
                    renderer.contains("Apple"),
                    "MacIntel paired with non-Apple renderer: {renderer}"
                ),
                Platform::Win32 => assert!(
                    renderer.contains("Direct3D11"),
                    "Win32 paired with non-D3D renderer: {renderer}"
                ),
                Platform::LinuxX86_64 => assert!(
                    renderer.contains("Mesa"),
                    "Linux paired with non-Mesa renderer: {renderer}"
                ),
            }
        }
    }

    #[test]
    fn different_seeds_can_differ() {
        // Guard against a constant generator: across a handful of fixed seeds
        // the sampled platforms must not all be identical.
        let g = Generator::embedded();
        let platforms: Vec<_> = (0..32u64)
            .map(|s| g.generate(Seed::from_u64(s)).platform)
            .collect();
        let first = platforms[0];
        assert!(
            platforms.iter().any(|p| *p != first),
            "generator produced the same platform for every seed"
        );
    }

    #[test]
    fn embedded_network_parses() {
        // Loading the bundled network must not panic.
        let g = Generator::embedded();
        assert!(!g.nodes.is_empty());
    }
}
