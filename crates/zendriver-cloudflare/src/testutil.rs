//! Test-only helpers shared by this crate's unit tests.
//!
//! Small on purpose: only the pieces every test module needs, so that a rule
//! about how to await a CDP command is written down once instead of being
//! re-derived per file.

#![allow(clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use zendriver_transport::testing::MockConnection;

/// How long a single awaited command may take to arrive before the test
/// fails.
///
/// [`MockConnection::expect_cmd`] has no timeout of its own, so a bare use of
/// it turns a refactor that drops a CDP call into a hang rather than a
/// failure — the test never fails, it just never finishes, and CI reports a
/// job timeout with no failing test name in it.
pub(crate) const CMD_BUDGET: Duration = Duration::from_secs(5);

/// Bounded [`MockConnection::expect_cmd`] — see [`CMD_BUDGET`].
///
/// Bounding it fixes the hang, not the weakness: `expect_cmd` silently
/// discards every frame that does not match, so it still only proves the
/// command *eventually* arrived. It can never establish that a command came
/// next, nor that some other command was absent. Assertions about ordering
/// or absence need a `recv_cmd_timeout` loop that sees every frame.
pub(crate) async fn expect_cmd(mock: &mut MockConnection, method: &str) -> u64 {
    match tokio::time::timeout(CMD_BUDGET, mock.expect_cmd(method)).await {
        Ok(id) => id,
        Err(_) => panic!("timed out waiting for {method}"),
    }
}
