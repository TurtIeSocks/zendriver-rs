//! Seed: controls deterministic farble. random (default) / from_system / explicit.

use serde::{Deserialize, Serialize};

/// A fingerprint seed. Serializes transparently as its u64 value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seed(pub u64);

impl Seed {
    /// Per-instance unique seed. THE DEFAULT.
    pub fn random() -> Seed {
        Seed(fastrand::u64(..))
    }

    /// Explicit reproducible seed.
    pub fn from_u64(v: u64) -> Seed {
        Seed(v)
    }

    /// Deterministic per-machine seed: stable across runs on the same host
    /// WITHOUT a user_data_dir. Opt-in — sticky per machine (one identity).
    pub fn from_system() -> Seed {
        Seed(system_seed_value())
    }

    /// Raw value for JS farble.
    pub fn value(self) -> u64 {
        self.0
    }
}

fn system_seed_value() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // Stable machine id, best-effort per OS; fall back to hostname + cpu brand.
    machine_id().hash(&mut h);
    sysinfo::System::host_name().unwrap_or_default().hash(&mut h);
    h.finish()
}

fn machine_id() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/etc/machine-id").unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        // IOPlatformUUID via ioreg; empty on failure (falls back to hostname).
        std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.lines()
                    .find(|l| l.contains("IOPlatformUUID"))
                    .map(|l| l.to_string())
            })
            .unwrap_or_default()
    }
    #[cfg(target_os = "windows")]
    {
        // MachineGuid from registry via reg query.
        std::process::Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Cryptography",
                "/v",
                "MachineGuid",
            ])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_u64_round_trips() {
        assert_eq!(Seed::from_u64(42).value(), 42);
    }

    #[test]
    fn from_system_is_stable_within_process() {
        assert_eq!(Seed::from_system(), Seed::from_system());
    }

    #[test]
    fn serde_is_transparent_u64() {
        let s = Seed::from_u64(7);
        assert_eq!(serde_json::to_string(&s).unwrap(), "7");
        let back: Seed = serde_json::from_str("7").unwrap();
        assert_eq!(back, s);
    }
}
