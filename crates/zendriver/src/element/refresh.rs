//! Auto-refresh: re-resolve a stale `Element` via its memoized
//! `ElementOrigin` and retry the wrapped CDP op once.
//!
//! P3 coverage:
//!   - `ElementOrigin::Query { scope_kind: TabMain, .. }` — re-runs the
//!     stored `SelectorKind::resolve_many` against `QueryScope::Tab` and
//!     re-picks the `nth` index.
//!   - `ElementOrigin::Query { scope_kind: ElementSubtree, .. }` —
//!     `NotRefreshable` in P3. Refreshing a subtree query requires
//!     reconstructing the parent element, which is the same
//!     traversal-chain refresh deferred to P4.
//!   - `ElementOrigin::Traversal { .. }` — `NotRefreshable` in P3.
//!     Full chain refresh lands in P4.
//!   - `ElementOrigin::Evaluation` — `NotRefreshable` (no selector to
//!     replay).
//!
//! `with_refresh(op)` retries `op` exactly once when the first attempt
//! errors with a stale-node signature (see `is_stale_node_error`). The
//! second failure surfaces as-is.

use std::future::Future;

use crate::element::{Element, ElementOrigin, ScopeKind};
use crate::error::{Result, ZendriverError};
use crate::query::selectors::QueryScope;

impl Element {
    /// Re-resolve this element's underlying CDP handle via its origin.
    /// Updates `backend_node_id` + `remote_object_id` in place on success.
    ///
    /// Returns `NotRefreshable` for `Evaluation`, `Traversal`, and
    /// `Query { ElementSubtree, .. }` origins in P3 (P4 lifts the
    /// traversal-chain restriction).
    pub async fn refresh(&self) -> Result<()> {
        let (new_backend, new_remote) = match &self.inner.origin {
            ElementOrigin::Query {
                scope_kind: ScopeKind::TabMain,
                selector,
                nth,
            } => {
                let tab = self.inner.tab.clone();
                let scope = QueryScope::Tab(&tab);
                let candidates = selector.resolve_many(&scope).await?;
                let r = candidates.into_iter().nth(*nth).ok_or_else(|| {
                    ZendriverError::ElementNotFound {
                        selector: format!("{selector:?}"),
                    }
                })?;
                (r.backend_node_id, r.remote_object_id)
            }
            ElementOrigin::Query {
                scope_kind: ScopeKind::ElementSubtree,
                ..
            }
            | ElementOrigin::Traversal { .. }
            | ElementOrigin::Evaluation => return Err(ZendriverError::NotRefreshable),
        };
        *self.inner.backend_node_id.lock().await = Some(new_backend);
        *self.inner.remote_object_id.lock().await = Some(new_remote);
        Ok(())
    }

    /// Run `op`, retrying it once if the first attempt errors with a
    /// stale-node signature. Used by every `Element` read + action so
    /// the retry-on-stale logic stays centralized.
    #[allow(dead_code)] // First callers land in T18 (reads) + T19+ (actions).
    pub(crate) async fn with_refresh<T, F, Fut>(&self, op: F) -> Result<T>
    where
        F: Fn() -> Fut + Send,
        Fut: Future<Output = Result<T>> + Send,
    {
        match op().await {
            Ok(v) => Ok(v),
            Err(e) if is_stale_node_error(&e) => {
                self.refresh().await?;
                op().await
            }
            Err(e) => Err(e),
        }
    }
}

/// Returns `true` if `e` looks like a stale-node failure from Chrome —
/// either the DOM domain reporting an unknown node id, or the Runtime
/// domain reporting a missing execution context (navigation race).
///
/// Matches against:
///   - `ZendriverError::ElementStale` (set by the inner-id accessors
///     when they observe a cleared id mid-flight).
///   - `ZendriverError::Navigation(m)` where `m` contains
///     `"No node with given id"` or `"Cannot find context"`. The
///     `From<CallError>` impl in `error.rs` maps `-32000 "Cannot find
///     context"` into the Navigation variant.
///   - `ZendriverError::Cdp { message, .. }` where `message` contains
///     either of the above substrings (covers DOM-domain stale errors
///     that don't get pre-mapped to Navigation).
#[allow(dead_code)] // First non-test caller is `with_refresh`, gated until T18.
pub(crate) fn is_stale_node_error(e: &ZendriverError) -> bool {
    match e {
        ZendriverError::ElementStale => true,
        ZendriverError::Navigation(m) => {
            m.contains("No node with given id") || m.contains("Cannot find context")
        }
        ZendriverError::Cdp { message, .. } => {
            message.contains("No node with given id") || message.contains("Cannot find context")
        }
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn is_stale_node_error_matches_expected_shapes() {
        // ElementStale: explicit stale signal.
        assert!(is_stale_node_error(&ZendriverError::ElementStale));

        // Navigation with "Cannot find context" — the `From<CallError>`
        // mapping in error.rs lands stale-context CDP errors here.
        assert!(is_stale_node_error(&ZendriverError::Navigation(
            "Cannot find context with specified id".into(),
        )));

        // Cdp variant with "No node with given id" — typical DOM-domain
        // stale error that isn't pre-mapped to Navigation.
        assert!(is_stale_node_error(&ZendriverError::Cdp {
            code: -32000,
            message: "No node with given id".into(),
            data: None,
        }));

        // Unrelated error: a plain timeout should NOT trigger refresh.
        assert!(!is_stale_node_error(&ZendriverError::Timeout(
            Duration::from_secs(1)
        )));
    }
}
