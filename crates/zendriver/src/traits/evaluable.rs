//! Shared JS-evaluation surface across types that own a CDP session/contextId.

use serde::de::DeserializeOwned;

use crate::error::Result;

/// Types that can evaluate JavaScript in their context.
///
/// Implemented by [`crate::Tab`] (main frame) and [`crate::Frame`]
/// (per-frame contextId). Element evaluation has a different shape
/// (binds `el` parameter) and has its own
/// [`crate::Element::evaluate`] / [`crate::Element::evaluate_main`]
/// inherent methods.
#[async_trait::async_trait]
pub trait Evaluable {
    /// Evaluate JS in an isolated world (sandbox; no page globals visible).
    /// Default for stealth-safe execution.
    async fn evaluate<T>(&self, js: &str) -> Result<T>
    where
        T: DeserializeOwned + Send + 'static;

    /// Evaluate JS in the main world (page globals accessible).
    async fn evaluate_main<T>(&self, js: &str) -> Result<T>
    where
        T: DeserializeOwned + Send + 'static;
}
