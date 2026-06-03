//! `browser_fingerprint_generate` — produce a Persona JSON from a real-device
//! source (pool / generative). Gated by the `fingerprints` cargo feature.

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where the persona comes from.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FpSource {
    /// Synthesize a coherent persona from the browserforge Bayesian network,
    /// downloaded on first use and cached locally.
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
    /// browserforge Bayesian network (downloaded + cached on first use); `pool`
    /// samples a downloaded real-device set (requires the published pool asset —
    /// see issue #25).
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

/// Resolve the generative network URL: `ZENDRIVER_FP_NETWORK_URL` if set, else the
/// crate default.
fn network_url() -> String {
    std::env::var("ZENDRIVER_FP_NETWORK_URL")
        .unwrap_or_else(|_| zendriver_fingerprints::generative::DEFAULT_NETWORK_URL.to_string())
}

/// Produce a [`GenerateOutput`] carrying a Persona JSON from the chosen
/// [`FpSource`].
///
/// `generative` synthesizes a persona from the browserforge Bayesian network,
/// which is downloaded on first use and cached. `pool` samples the published
/// real-device dataset (also downloaded on first use) — and, until the pool asset
/// is hosted (issue #25), surfaces an `internal_error`. An optional `seed` makes
/// either source reproducible. Takes no `SessionState` — fingerprint generation
/// is browser-independent.
pub async fn generate(input: GenerateInput) -> Result<GenerateOutput, ErrorData> {
    generate_from(input, &network_url()).await
}

/// Inner generator with an injectable generative network URL (test seam).
async fn generate_from(
    input: GenerateInput,
    network_url: &str,
) -> Result<GenerateOutput, ErrorData> {
    use zendriver::Seed;
    let seed = input.seed.map_or_else(Seed::random, Seed::from_u64);
    let persona = match input.source {
        FpSource::Generative => {
            zendriver_fingerprints::generative::Generator::load_or_download(network_url)
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("generative network load failed: {e}"), None)
                })?
                .generate(seed)
        }
        FpSource::Pool => {
            // NOTE: The pool release asset does not exist yet (tracked in issue
            // #25). This fails at runtime with a clear error until it is hosted.
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn generative_via_mock_is_non_null_and_deterministic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(zendriver_fingerprints::generative::TEST_NETWORK_ZIP),
            )
            .mount(&server)
            .await;

        let a = generate_from(
            GenerateInput {
                source: FpSource::Generative,
                seed: Some(7),
            },
            &server.uri(),
        )
        .await
        .expect("a");
        assert!(a.persona.is_object());

        let b = generate_from(
            GenerateInput {
                source: FpSource::Generative,
                seed: Some(7),
            },
            &server.uri(),
        )
        .await
        .expect("b");
        assert_eq!(a.persona, b.persona);
    }
}
