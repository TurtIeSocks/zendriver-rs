//! `Element` — handle to a DOM node via CDP `RemoteObjectId` / `BackendNodeId`.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::error::{Result, ZendriverError};
use crate::tab::Tab;

#[derive(Clone)]
pub struct Element {
    pub(crate) inner: Arc<ElementInner>,
}

pub(crate) struct ElementInner {
    pub(crate) tab: Tab,
    // Held for future tasks (e.g. DOM-domain calls keyed by backendNodeId).
    #[allow(dead_code)]
    pub(crate) backend_node_id: i64,
    pub(crate) remote_object_id: String,
}

impl Element {
    // Constructor consumed by Task 23 (`FindBuilder` materializes Elements);
    // no Phase 1 caller yet.
    #[allow(dead_code)]
    pub(crate) fn new(tab: Tab, backend_node_id: i64, remote_object_id: String) -> Self {
        Self {
            inner: Arc::new(ElementInner {
                tab,
                backend_node_id,
                remote_object_id,
            }),
        }
    }

    // Accessor consumed by later phases; no Phase 1 caller yet.
    #[allow(dead_code)]
    pub(crate) fn tab(&self) -> &Tab {
        &self.inner.tab
    }

    /// Call a JS function on this element's remote object. The function
    /// signature MUST take exactly one parameter (the element); use
    /// `function(el){ ... }`.
    pub(crate) async fn call_on(&self, function: &str, args: Value) -> Result<Value> {
        let res = self
            .inner
            .tab
            .call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": self.inner.remote_object_id,
                    "functionDeclaration": function,
                    "arguments": args,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        if let Some(details) = res.get("exceptionDetails") {
            let msg = details
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(|d| d.as_str())
                .unwrap_or("unknown")
                .to_string();
            return Err(ZendriverError::JsException(msg));
        }
        Ok(res["result"].clone())
    }

    /// Evaluate a JS expression where `el` is bound to this element handle.
    // Public API consumed by later phases; no Phase 1 caller yet.
    #[allow(dead_code)]
    pub async fn evaluate<T: DeserializeOwned>(&self, js: impl AsRef<str>) -> Result<T> {
        let function = format!("function(el){{ return ({}) }}", js.as_ref());
        let result = self
            .call_on(
                &function,
                json!([{ "objectId": self.inner.remote_object_id }]),
            )
            .await?;
        let value = result.get("value").cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(ZendriverError::Serde)
    }
}
