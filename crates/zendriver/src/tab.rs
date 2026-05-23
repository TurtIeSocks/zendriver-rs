//! Tab — handle to a single CDP target session.

use std::sync::Arc;

use serde_json::Value;
use tracing::trace;
use zendriver_transport::SessionHandle;

use crate::error::Result;

#[derive(Clone)]
pub struct Tab {
    pub(crate) inner: Arc<TabInner>,
}

pub(crate) struct TabInner {
    pub(crate) session: SessionHandle,
}

impl Tab {
    pub(crate) fn new(session: SessionHandle) -> Self {
        Self {
            inner: Arc::new(TabInner { session }),
        }
    }

    /// Escape hatch: raw `SessionHandle` for advanced users who need to send
    /// CDP commands the high-level API doesn't expose.
    pub fn session(&self) -> &SessionHandle {
        &self.inner.session
    }

    /// Helper: call a CDP method on this tab's session, parsing transport
    /// errors into `ZendriverError`.
    #[allow(dead_code)] // consumed by T17/T18/T19
    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value> {
        trace!(%method, "tab.call");
        let res = self.inner.session.call(method, params).await?;
        Ok(res)
    }
}
