//! `browser_fingerprint_generate` — produce a Persona JSON from a real-device
//! source (pool / generative). Gated by the `fingerprints` cargo feature.

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where the persona comes from.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FpSource {
    /// Synthesize a coherent persona from the embedded Bayesian network. Works
    /// offline; no download needed.
    Generative,
    /// Sample a real-device persona from the published pool dataset. Requires
    /// the pool asset to be downloadable — see POOL_URL and issue #25.
    Pool,
}

/// Input for `browser_fingerprint_generate`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerateInput {
    /// Where the persona comes from. `generative` synthesizes one from the
    /// embedded Bayesian network (offline); `pool` samples a downloaded
    /// real-device set (requires the published pool asset — see issue #25).
    pub source: FpSource,
    /// Optional seed for reproducibility. Omit for a random persona.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// Output for `browser_fingerprint_generate`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GenerateOutput {
    /// A Persona JSON — pass to `browser_open`'s `persona` field (inspect /
    /// tweak the JSON first if desired).
    pub persona: serde_json::Value,
}

// TODO(#25): real asset URL once the fingerprint-pool release asset is published.
const POOL_URL: &str =
    "https://github.com/TurtIeSocks/zendriver-rs/releases/latest/download/fingerprint-pool.json";

/// Produce a [`GenerateOutput`] carrying a Persona JSON from the chosen
/// [`FpSource`].
///
/// `generative` synthesizes a persona from the embedded Bayesian network
/// (offline, no download). `pool` samples the published real-device dataset,
/// which is downloaded on first use — and, until the pool asset is hosted
/// (issue #25), surfaces an `internal_error` explaining the gap. An optional
/// `seed` makes either source reproducible. Takes no `SessionState` — fingerprint
/// generation is browser-independent.
pub async fn generate(input: GenerateInput) -> Result<GenerateOutput, ErrorData> {
    use zendriver::Seed;
    let seed = input.seed.map_or_else(Seed::random, Seed::from_u64);
    let persona = match input.source {
        FpSource::Generative => {
            zendriver_fingerprints::generative::Generator::embedded().generate(seed)
        }
        FpSource::Pool => {
            // NOTE: The pool release asset does not exist yet (tracked in issue
            // #25 — the dataset has not been published). This will fail at
            // runtime with a clear error until the asset is hosted.
            let set = zendriver_fingerprints::pool::load_or_download(POOL_URL)
                .await
                .map_err(|e| {
                    ErrorData::internal_error(
                        format!(
                            "pool load failed (the pool asset may not be published yet — see issue #25): {e}"
                        ),
                        None,
                    )
                })?;
            set.sample(seed)
        }
    };
    let value = serde_json::to_value(&persona)
        .map_err(|e| ErrorData::internal_error(format!("persona serialize: {e}"), None))?;
    Ok(GenerateOutput { persona: value })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn generative_produces_non_null_persona() {
        let out = generate(GenerateInput {
            source: FpSource::Generative,
            seed: Some(42),
        })
        .await
        .expect("generative generate");
        // persona must be a non-null JSON object
        assert!(
            out.persona.is_object(),
            "expected object, got {:?}",
            out.persona
        );
    }

    #[tokio::test]
    async fn generative_is_deterministic() {
        let a = generate(GenerateInput {
            source: FpSource::Generative,
            seed: Some(7),
        })
        .await
        .expect("a");
        let b = generate(GenerateInput {
            source: FpSource::Generative,
            seed: Some(7),
        })
        .await
        .expect("b");
        assert_eq!(a.persona, b.persona);
    }
}
