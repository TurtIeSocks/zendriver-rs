//! Read-only state inspectors — `browser_element_state`.
//!
//! Resolves an element via the [`crate::tools::find`] bridge, then
//! populates a sparse `ElementState` per the requested `include` preset.
//! Every populated field is `Option<_>` so unrequested fields can be
//! dropped from the wire payload via `skip_serializing_if`.

use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::{McpServerError, map_error};
use crate::selectors::Selector;
use crate::state::SessionState;
use crate::tools::common::current_tab;
use crate::tools::find::{BoundingBox, resolve};

/// Field preset for `browser_element_state`.
///
/// Each preset chooses which subset of the (potentially expensive) probes
/// the handler runs. Defaults to [`ReadFieldsPreset::All`] — costs an
/// extra eval + attrs + inner_html call but matches the most common
/// agent inspection flow.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadFieldsPreset {
    /// Every field (existence, visible/enabled, geometry, text/attrs/html).
    /// `in_viewport` remains `None` in v0 — no zendriver primitive yet.
    All,
    /// Only `exists`. Cheapest preset — skips every probe past resolve.
    ExistsOnly,
    /// `exists`, `visible`, `enabled`.
    VisibleEnabled,
    /// `exists`, `bounding_box`. `in_viewport` stays `None` in v0.
    Geometry,
    /// `exists`, `text`, `attrs`, `inner_html`.
    TextAttrs,
}

const fn default_preset() -> ReadFieldsPreset {
    ReadFieldsPreset::All
}

/// Input for `browser_element_state`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ElementStateInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// Field-subset preset. See [`ReadFieldsPreset`]. Default: `all`.
    #[serde(default = "default_preset")]
    pub include: ReadFieldsPreset,
}

/// Output of `browser_element_state`. Every field is `Option<_>` so only
/// the ones a preset requested round-trip on the wire.
#[derive(Debug, Serialize, JsonSchema, Default)]
pub struct ElementState {
    /// `true` when the selector resolved to an element. Always set (every
    /// preset includes this).
    pub exists: bool,
    /// `true` when the lib's visibility predicate accepts the element.
    /// Populated by `All` and `VisibleEnabled`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// `true` when the element is not disabled. Populated by `All` and
    /// `VisibleEnabled`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `true` when the element's bounding box intersects the viewport.
    /// Always `None` in v0 — no zendriver primitive yet. Field reserved
    /// so a follow-up dispatch can add it without breaking the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_viewport: Option<bool>,
    /// Viewport-relative bounding box. Populated by `All` and `Geometry`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<BoundingBox>,
    /// `innerText`. Populated by `All` and `TextAttrs`. NOT truncated —
    /// callers wanting a snippet should clip on their side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// All HTML attributes. Populated by `All` and `TextAttrs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attrs: Option<BTreeMap<String, String>>,
    /// Serialized `innerHTML`. Populated by `All` and `TextAttrs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner_html: Option<String>,
}

/// Probe an element's state per the requested field preset.
///
/// `exists: false` short-circuits to skip every other probe, so a
/// missing-element check costs exactly one find dispatch.
pub async fn element_state(
    state: Arc<Mutex<SessionState>>,
    input: ElementStateInput,
) -> Result<ElementState, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let el = match resolve(&tab, &input.selector).await {
        Ok(el) => el,
        Err(err) if is_not_found(&err) => {
            // Missing element → exists=false, no other fields.
            return Ok(ElementState {
                exists: false,
                ..Default::default()
            });
        }
        Err(err) => return Err(err),
    };
    let mut out = ElementState {
        exists: true,
        ..Default::default()
    };
    let want_visible = matches!(
        input.include,
        ReadFieldsPreset::All | ReadFieldsPreset::VisibleEnabled
    );
    let want_geometry = matches!(
        input.include,
        ReadFieldsPreset::All | ReadFieldsPreset::Geometry
    );
    let want_text_attrs = matches!(
        input.include,
        ReadFieldsPreset::All | ReadFieldsPreset::TextAttrs
    );

    if want_visible {
        let visible = el
            .is_visible()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
        let enabled = el
            .is_enabled()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
        out.visible = Some(visible);
        out.enabled = Some(enabled);
    }
    if want_geometry {
        // `bounding_box` swallows `display:none` to `None` already; we
        // surface that as a missing field rather than an error.
        let bbox = el
            .bounding_box()
            .await
            .ok()
            .flatten()
            .map(BoundingBox::from);
        out.bounding_box = bbox;
        // `in_viewport` reserved for v1 — no zendriver primitive yet.
        out.in_viewport = None;
    }
    if want_text_attrs {
        let text = el
            .inner_text()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
        let attrs_map = el
            .attrs()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
        let inner_html = el
            .inner_html()
            .await
            .map_err(|e| map_error(McpServerError::from(e)))?;
        out.text = Some(text);
        out.attrs = Some(attrs_map.into_iter().collect());
        out.inner_html = Some(inner_html);
    }
    Ok(out)
}

/// Mirror of the same predicate in [`crate::tools::find`] — kept private
/// because the only consumer is this module's "missing element" branch.
fn is_not_found(err: &ErrorData) -> bool {
    err.data
        .as_ref()
        .and_then(|v| v.get("suggested_next"))
        .and_then(|v| v.as_str())
        == Some("browser_snapshot")
        && err.message.contains("No element matched")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selectors::Selector;

    fn fresh() -> Arc<Mutex<SessionState>> {
        Arc::new(Mutex::new(SessionState::new()))
    }

    fn css_sel(s: &str) -> Selector {
        Selector {
            css: Some(s.into()),
            xpath: None,
            text: None,
            text_exact: None,
            text_regex: None,
            role: None,
            role_name: None,
            nth: None,
            visible_only: true,
            timeout_ms: 5000,
            frame_id: None,
        }
    }

    #[tokio::test]
    async fn element_state_with_no_browser_suggests_browser_open() {
        let err = element_state(
            fresh(),
            ElementStateInput {
                selector: css_sel("h1"),
                include: ReadFieldsPreset::All,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[test]
    fn default_preset_is_all() {
        assert_eq!(default_preset(), ReadFieldsPreset::All);
    }
}
