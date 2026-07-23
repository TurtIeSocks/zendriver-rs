//! Proves a caller can implement a custom `TargetObserver` using only
//! `zendriver`'s public re-exports, without depending on `zendriver-transport`
//! directly — the exact use case SEMVER.md's "depend on zendriver instead"
//! guidance previously couldn't satisfy (the observer surface wasn't hoisted).
//!
//! This is a compile-time proof as much as a runtime one: if any of the hoisted
//! observer types were missing from the facade, this file would fail to compile.

use zendriver::{ObserverError, ObserverFailurePolicy, PausedSession, TargetObserver};

struct NoOpObserver;

#[async_trait::async_trait]
impl TargetObserver for NoOpObserver {
    async fn on_target_attached(&self, _session: PausedSession<'_>) -> Result<(), ObserverError> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "noop-observer-facade-smoke-test"
    }

    fn failure_policy(&self) -> ObserverFailurePolicy {
        ObserverFailurePolicy::BestEffort
    }
}

#[test]
fn target_observer_is_implementable_via_the_facade_alone() {
    let observer = NoOpObserver;
    assert_eq!(observer.name(), "noop-observer-facade-smoke-test");
    assert_eq!(observer.failure_policy(), ObserverFailurePolicy::BestEffort);
}
