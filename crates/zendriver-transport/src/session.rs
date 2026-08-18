//! `SessionHandle`: a [`Connection`] bound to a particular CDP session.
//!
//! - A *flat* handle ([`SessionHandle::new`]) rides a shared, flattened browser
//!   socket and tags every frame with its `sessionId`.
//! - A *root* handle ([`SessionHandle::new_root`]) owns a connection dialed
//!   straight at one target (e.g. `/devtools/page/<targetId>`), so it is that
//!   target's session and carries no `sessionId` — the one-socket-per-tab model.

use std::sync::Arc;

use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::connection::Connection;
use crate::error::CallError;
use crate::frame::RawEvent;

/// Cheap-to-clone handle binding a [`Connection`] to a specific CDP session.
///
/// Every CDP target — page, OOPIF, worker — has its own `sessionId` after the
/// browser fires `Target.attachedToTarget`. A `SessionHandle` couples a shared
/// transport with one such id so callers can issue commands without repeating
/// the id on every call.
#[derive(Clone, Debug)]
pub struct SessionHandle {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    conn: Connection,
    /// `Some(id)` for a flat session; `None` for a root (per-target) session.
    session_id: Option<String>,
}

impl SessionHandle {
    /// Construct a flat handle around `conn` scoped to `session_id` (a session
    /// multiplexed over a shared, flattened browser socket).
    pub fn new(conn: Connection, session_id: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                conn,
                session_id: Some(session_id.into()),
            }),
        }
    }

    /// Construct a root handle around a connection dialed directly at one target
    /// (e.g. `/devtools/page/<targetId>`): commands carry no `sessionId` and
    /// event filtering matches frames that likewise carry none.
    pub fn new_root(conn: Connection) -> Self {
        Self {
            inner: Arc::new(Inner {
                conn,
                session_id: None,
            }),
        }
    }

    /// The CDP `sessionId` this handle is scoped to, or `""` for a root session.
    pub fn session_id(&self) -> &str {
        self.inner.session_id.as_deref().unwrap_or("")
    }

    /// Whether this is a root (per-target connection) handle.
    pub fn is_root(&self) -> bool {
        self.inner.session_id.is_none()
    }

    /// Borrow the underlying [`Connection`].
    pub fn connection(&self) -> &Connection {
        &self.inner.conn
    }

    /// Send a CDP command routed to this session. A flat session tags the frame
    /// with its `sessionId`; a root session sends none.
    pub async fn call(&self, method: impl Into<String>, params: Value) -> Result<Value, CallError> {
        self.inner
            .conn
            .call_raw(method, params, self.inner.session_id.clone())
            .await
    }

    /// Subscribe to events of type `T` on this session. Matches a flat session's
    /// `sessionId`, or (for a root session) frames that carry none.
    pub fn subscribe<T>(
        &self,
        method: &'static str,
    ) -> impl Stream<Item = T> + Send + Unpin + use<T>
    where
        T: DeserializeOwned + Send + 'static,
    {
        let sid = self.inner.session_id.clone();
        let raw = self.inner.conn.subscribe_raw();
        Box::pin(raw.filter_map(move |ev: RawEvent| {
            let matches = ev.session_id == sid && ev.method == method;
            async move {
                matches
                    .then(|| serde_json::from_value(ev.params).ok())
                    .flatten()
            }
        }))
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::connection::spawn_actor;
    use crate::connection::test_only::duplex_pair;
    use serde_json::json;
    use tokio_tungstenite::tungstenite::Message;

    #[tokio::test]
    async fn session_call_includes_session_id() {
        let (ws, _test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        let sess = SessionHandle::new(conn.clone(), "S1");

        let call = tokio::spawn({
            let s = sess.clone();
            async move {
                s.call("Page.navigate", json!({ "url": "https://x.test" }))
                    .await
            }
        });

        let sent = test_rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(match &sent {
            Message::Text(t) => t,
            _ => panic!("expected text frame"),
        })
        .unwrap();
        assert_eq!(v["sessionId"], "S1");

        // Cancel via dropping
        drop(call);
        conn.shutdown();
    }

    #[tokio::test]
    async fn root_session_omits_session_id() {
        let (ws, _test_tx, mut test_rx) = duplex_pair();
        let conn = spawn_actor(ws);
        let sess = SessionHandle::new_root(conn.clone());
        assert!(sess.is_root());
        assert_eq!(sess.session_id(), "");

        let call = tokio::spawn({
            let s = sess.clone();
            async move {
                s.call("Page.navigate", json!({ "url": "https://x.test" }))
                    .await
            }
        });

        let sent = test_rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(match &sent {
            Message::Text(t) => t,
            _ => panic!("expected text frame"),
        })
        .unwrap();
        // A direct per-target connection carries no sessionId on the wire.
        assert!(v.get("sessionId").is_none());

        drop(call);
        conn.shutdown();
    }
}
