//! Tab — handle to a single CDP target session.

use std::sync::Arc;
use zendriver_transport::SessionHandle;

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

    pub fn session(&self) -> &SessionHandle {
        &self.inner.session
    }
}
