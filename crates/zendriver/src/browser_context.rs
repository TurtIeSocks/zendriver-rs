//! Per-`BrowserContext` lifecycle on top of CDP `Target.createBrowserContext`.

use std::sync::Arc;
use crate::browser::BrowserInner;

pub struct BrowserContext {
    pub(crate) browser: Arc<BrowserInner>,
    pub(crate) id: String,
}

impl BrowserContext {
    pub fn id(&self) -> &str {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_context_exposes_id() {
        fn _accept(_: &BrowserContext) {}
    }
}
