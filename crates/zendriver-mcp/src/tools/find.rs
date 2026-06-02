//! Element discovery handlers — `browser_find`, `browser_find_all`.
//!
//! Also hosts the [`Selector`][crate::selectors::Selector] →
//! [`zendriver::FindBuilder`] bridge ([`resolve`] / [`resolve_all`]) consumed
//! by every tool that takes a Selector arg block. The bridge lives here
//! (rather than on `Selector` itself) because it depends on zendriver types
//! that `selectors.rs` deliberately doesn't pull in — keeping the wire
//! struct decoupled from CDP-shaped concerns.
//!
//! ## Frame routing
//!
//! When the caller sets `selector.frame_id`, we look up the matching
//! [`zendriver::Frame`] from `tab.frames()` and call `.in_frame(&frame)`
//! on the builder. Missing frame id maps to
//! [`zendriver::ZendriverError::FrameNotFound`].
//!
//! ## ARIA role mapping
//!
//! The wire `role: String` is mapped to [`zendriver::AriaRole`] via a
//! match against the same shortlist the lib enumerates. Unknown role
//! strings yield [`rmcp::ErrorData::invalid_params`] listing the accepted
//! variants — surfacing the typo to the agent rather than silently falling
//! back to `AriaRole::Other` (which would compile to a literal
//! `[role="<typo>"]` selector that matches nothing).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zendriver::query::FindAllBuilder;
use zendriver::{AriaRole, Element, FindBuilder, Tab};

use crate::errors::{McpServerError, map_error};
use crate::selectors::{AttrOp, Selector};
use crate::state::SessionState;
use crate::tools::common::{current_tab, lookup_frame};

// ---------- Selector → FindBuilder bridge --------------------------------

/// Resolve a single element by [`Selector`].
///
/// Validates the selector, configures a [`FindBuilder`] with the chosen
/// selector kind + modifiers (`nth`, `visible_only`, `timeout`,
/// `in_frame`), then awaits `.one()`. Surfaces
/// [`zendriver::ZendriverError::ElementNotFound`] to the caller — tools that want to
/// swallow it (e.g. `browser_find` returning `found: false`) must do so
/// themselves; the helper does NOT call `one_or_none` for them.
pub async fn resolve(tab: &Tab, sel: &Selector) -> Result<Element, ErrorData> {
    sel.validate()
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
    // Frame lookup must happen BEFORE the builder is constructed so the
    // borrow checker is happy — `in_frame(&frame)` takes a reference, and
    // we need a stable owned `Frame` to borrow from.
    let frame_handle = if let Some(fid) = sel.frame_id.as_deref() {
        Some(lookup_frame(tab, fid).await?)
    } else {
        None
    };
    let builder = tab.find();
    let builder = apply_selector(builder, sel)?;
    let builder = apply_modifiers(builder, sel);
    let builder = if let Some(ref f) = frame_handle {
        builder.in_frame(f)
    } else {
        builder
    };
    builder
        .one()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))
}

/// Resolve up to `limit` elements by [`Selector`].
///
/// Same bridge as [`resolve`], but terminates with `.many_or_empty()` so
/// "no matches" returns `Vec::new()` instead of an error. The caller's
/// `limit` is applied after the lib returns its full match list — we don't
/// have a per-call cap to push into the builder.
pub async fn resolve_all(
    tab: &Tab,
    sel: &Selector,
    limit: usize,
) -> Result<Vec<Element>, ErrorData> {
    sel.validate()
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
    let frame_handle = if let Some(fid) = sel.frame_id.as_deref() {
        Some(lookup_frame(tab, fid).await?)
    } else {
        None
    };
    let builder = tab.find_all();
    let builder = apply_selector_all(builder, sel)?;
    let builder = apply_modifiers_all(builder, sel);
    let builder = if let Some(ref f) = frame_handle {
        builder.in_frame(f)
    } else {
        builder
    };
    let mut all = builder
        .many_or_empty()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    if all.len() > limit {
        all.truncate(limit);
    }
    Ok(all)
}

/// Apply a [`Selector`]'s chosen kind onto a query builder, returning the
/// configured builder (or an `invalid_params` `ErrorData`).
///
/// Generated once per builder type ([`FindBuilder`] for single-match,
/// [`FindAllBuilder`] for multi-match) because the lib exposes the two as
/// distinct concrete types with no shared trait — but they expose the
/// *identical* selector/predicate method surface, so a single macro body
/// covers both and a new selector kind only has to be added here once.
///
/// Two modes (mutually exclusive — enforced by [`Selector::validate`]):
/// - **single-selector** (`css`/`xpath`/`text`/`text_exact`/`text_regex`/`role`)
///   — exactly one is applied.
/// - **predicate** (`tag` and/or `attrs`, plus optional `text*` post-filters,
///   all AND-ed) — applies `.tag()`, the `.attr*()` / `.has_attr()` /
///   `.attr_regex()` family, and `.containing_text()` / `.text_equals()` /
///   `.text_matches()`.
macro_rules! apply_selector_to {
    ($builder:expr, $sel:expr) => {{
        let mut builder = $builder;
        let sel = $sel;
        // Predicate mode: tag/attrs present; text* are optional AND-ed
        // post-filters. Single-selector kinds (css/xpath/role) are absent
        // here (validate() enforces the mutual exclusion).
        if sel.tag.is_some() || !sel.attrs.is_empty() {
            if let Some(tag) = sel.tag.as_deref() {
                builder = builder.tag(tag);
            }
            for ap in &sel.attrs {
                let name = ap.name.as_str();
                let value = ap.value.as_deref().unwrap_or(""); // Has has no value
                builder = match ap.op {
                    AttrOp::Eq => builder.attr(name, value),
                    AttrOp::Contains => builder.attr_contains(name, value),
                    AttrOp::StartsWith => builder.attr_starts_with(name, value),
                    AttrOp::EndsWith => builder.attr_ends_with(name, value),
                    AttrOp::Has => builder.has_attr(name),
                    AttrOp::Regex => builder.attr_regex(name, value),
                };
            }
            if let Some(t) = sel.text.as_deref() {
                builder = builder.containing_text(t);
            }
            if let Some(t) = sel.text_exact.as_deref() {
                builder = builder.text_equals(t);
            }
            if let Some(pat) = sel.text_regex.as_deref() {
                builder = builder.text_matches(pat);
            }
            Ok(builder)
        } else if let Some(css) = sel.css.as_deref() {
            Ok(builder.css(css))
        } else if let Some(xp) = sel.xpath.as_deref() {
            Ok(builder.xpath(xp))
        } else if let Some(t) = sel.text.as_deref() {
            Ok(builder.text(t))
        } else if let Some(t) = sel.text_exact.as_deref() {
            Ok(builder.text_exact(t))
        } else if let Some(pat) = sel.text_regex.as_deref() {
            let re = compile_regex(pat)?;
            Ok(builder.text_regex(re))
        } else if let Some(role_str) = sel.role.as_deref() {
            let role = parse_role(role_str)?;
            Ok(match sel.role_name.as_deref() {
                Some(name) => builder.role_named(role, name),
                None => builder.role(role),
            })
        } else {
            // `validate()` already enforces exactly-one-of, so this branch is
            // unreachable in practice — return invalid_params just in case.
            Err(ErrorData::invalid_params(
                "Selector has no selector kind set (validate() should have caught this)"
                    .to_string(),
                None,
            ))
        }
    }};
}

/// Apply the Selector's chosen kind to a single-match [`FindBuilder`].
fn apply_selector<'a>(
    builder: FindBuilder<'a>,
    sel: &Selector,
) -> Result<FindBuilder<'a>, ErrorData> {
    apply_selector_to!(builder, sel)
}

/// Apply the Selector's chosen kind to a [`FindAllBuilder`].
fn apply_selector_all<'a>(
    builder: FindAllBuilder<'a>,
    sel: &Selector,
) -> Result<FindAllBuilder<'a>, ErrorData> {
    apply_selector_to!(builder, sel)
}

fn apply_modifiers<'a>(mut builder: FindBuilder<'a>, sel: &Selector) -> FindBuilder<'a> {
    if let Some(n) = sel.nth {
        builder = builder.nth(n);
    }
    builder = builder.visible_only(sel.visible_only);
    builder = builder.timeout(Duration::from_millis(sel.timeout_ms));
    builder
}

fn apply_modifiers_all<'a>(mut builder: FindAllBuilder<'a>, sel: &Selector) -> FindAllBuilder<'a> {
    builder = builder.visible_only(sel.visible_only);
    builder = builder.timeout(Duration::from_millis(sel.timeout_ms));
    builder
}

fn compile_regex(pat: &str) -> Result<regex::Regex, ErrorData> {
    regex::Regex::new(pat)
        .map_err(|e| ErrorData::invalid_params(format!("Invalid `text_regex` pattern: {e}"), None))
}

/// Map the wire `role` string onto a [`zendriver::AriaRole`].
///
/// We accept lowercase ARIA role names. Unknown values surface an
/// `invalid_params` error listing the accepted roles, so an agent that
/// typos `"buttn"` gets a clean signal rather than `AriaRole::Other`
/// silently compiling to a `[role="buttn"]` selector that matches nothing.
fn parse_role(s: &str) -> Result<AriaRole, ErrorData> {
    match s {
        "button" => Ok(AriaRole::Button),
        "link" => Ok(AriaRole::Link),
        "textbox" => Ok(AriaRole::Textbox),
        "combobox" => Ok(AriaRole::Combobox),
        "checkbox" => Ok(AriaRole::Checkbox),
        "radio" => Ok(AriaRole::Radio),
        "tab" => Ok(AriaRole::Tab),
        "menu" => Ok(AriaRole::Menu),
        "menuitem" => Ok(AriaRole::Menuitem),
        "dialog" => Ok(AriaRole::Dialog),
        "heading" => Ok(AriaRole::Heading),
        "banner" => Ok(AriaRole::Banner),
        "navigation" => Ok(AriaRole::Navigation),
        "main" => Ok(AriaRole::Main),
        "article" => Ok(AriaRole::Article),
        "list" => Ok(AriaRole::List),
        "listitem" => Ok(AriaRole::Listitem),
        "row" => Ok(AriaRole::Row),
        "cell" => Ok(AriaRole::Cell),
        "columnheader" => Ok(AriaRole::Columnheader),
        "rowheader" => Ok(AriaRole::Rowheader),
        other => Err(ErrorData::invalid_params(
            format!(
                "Unknown ARIA role `{other}`. Accepted: button, link, textbox, combobox, checkbox, radio, tab, menu, menuitem, dialog, heading, banner, navigation, main, article, list, listitem, row, cell, columnheader, rowheader."
            ),
            None,
        )),
    }
}

// ---------- ElementDescriptor / BoundingBox -------------------------------

/// Geometry wire shape mirroring [`zendriver::BoundingBox`].
///
/// Wrapped (rather than re-exporting the lib type) so it can derive
/// `Serialize` + `JsonSchema` without forcing those derives on the lib's
/// type.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema, PartialEq)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<zendriver::BoundingBox> for BoundingBox {
    fn from(b: zendriver::BoundingBox) -> Self {
        Self {
            x: b.x,
            y: b.y,
            width: b.width,
            height: b.height,
        }
    }
}

/// Element projection returned by `browser_find` (single) and
/// `browser_find_all` (list).
///
/// `tag` is best-effort: the lib has no `Element::tag_name()` so we ask
/// the page via `el.tagName.toLowerCase()`. If the eval call fails (e.g.
/// the element became stale between the find and the describe), we leave
/// `tag` as `None` rather than failing the entire describe — the rest of
/// the projection (attrs, text, geometry) is still useful.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ElementDescriptor {
    /// Lowercase tag name, or `None` when the eval probe failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// First 200 chars of `innerText` (post `chars().take(200)` so we
    /// never split a multibyte codepoint).
    pub text_snippet: String,
    /// All attributes on the element, in stable key order.
    pub attrs: BTreeMap<String, String>,
    /// Viewport-relative bounding box, or `None` when the element has no
    /// box (display: none, detached, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<BoundingBox>,
    /// `true` when the lib's visibility predicate accepts the element.
    pub visible: bool,
    /// `true` when the element is not disabled (form-aware).
    pub enabled: bool,
}

/// Build a descriptor from a live `Element`.
///
/// Attribute / visibility / enabled probes propagate their errors (those
/// failures usually mean the element went stale and the caller should
/// re-find). The tag probe is intentionally swallowed to `None` — see
/// the [`ElementDescriptor::tag`] doc for the rationale.
pub async fn describe(el: &Element) -> Result<ElementDescriptor, ErrorData> {
    // Best-effort tag probe — falls back to None on any failure.
    let tag: Option<String> = el.evaluate::<String>("el.tagName.toLowerCase()").await.ok();

    // Text snippet: cap at 200 chars (not bytes) so we never split a
    // multibyte codepoint, which would yield invalid UTF-8 on output.
    let text_snippet: String = el
        .inner_text()
        .await
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();

    let attrs_map = el
        .attrs()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    let attrs: BTreeMap<String, String> = attrs_map.into_iter().collect();

    let visible = el
        .is_visible()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    let enabled = el
        .is_enabled()
        .await
        .map_err(|e| map_error(McpServerError::from(e)))?;
    let bounding_box = el
        .bounding_box()
        .await
        .ok()
        .flatten()
        .map(BoundingBox::from);

    Ok(ElementDescriptor {
        tag,
        text_snippet,
        attrs,
        bounding_box,
        visible,
        enabled,
    })
}

// ---------- browser_find --------------------------------------------------

/// Input for `browser_find`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindInput {
    #[serde(flatten)]
    pub selector: Selector,
}

/// Output of `browser_find`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FindOutput {
    /// `true` iff the selector matched within the timeout.
    pub found: bool,
    /// Element projection when `found` is `true`, otherwise `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<ElementDescriptor>,
}

/// Resolve a Selector to a single element and return its descriptor.
///
/// Distinct from `browser_element_state` in that "not found" is reported
/// as `{ found: false, element: null }` rather than an error — easier for
/// agents that want to branch on existence without try/catch.
pub async fn find(
    state: Arc<Mutex<SessionState>>,
    input: FindInput,
) -> Result<FindOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    match resolve(&tab, &input.selector).await {
        Ok(el) => {
            let desc = describe(&el).await?;
            Ok(FindOutput {
                found: true,
                element: Some(desc),
            })
        }
        Err(err) if is_not_found(&err) => Ok(FindOutput {
            found: false,
            element: None,
        }),
        Err(err) => Err(err),
    }
}

/// `true` when the `ErrorData` was produced by mapping an
/// [`zendriver::ZendriverError::ElementNotFound`]. We rely on the
/// `_meta.suggested_next == "browser_snapshot"` marker that `map_error`
/// attaches to that variant — checking the message would be brittle
/// across translation tweaks.
fn is_not_found(err: &ErrorData) -> bool {
    err.data
        .as_ref()
        .and_then(|v| v.get("suggested_next"))
        .and_then(|v| v.as_str())
        == Some("browser_snapshot")
        && err.message.contains("No element matched")
}

// ---------- browser_find_all ----------------------------------------------

/// Input for `browser_find_all`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindAllInput {
    #[serde(flatten)]
    pub selector: Selector,
    /// Maximum number of matches to return. Default 50.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

const fn default_limit() -> usize {
    50
}

/// Output of `browser_find_all`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FindAllOutput {
    /// All matched elements (capped at `limit`).
    pub elements: Vec<ElementDescriptor>,
}

/// Resolve a Selector to ALL matches (up to `limit`).
///
/// "No matches" returns `elements: []` rather than an error — keeps the
/// caller's branching uniform.
pub async fn find_all(
    state: Arc<Mutex<SessionState>>,
    input: FindAllInput,
) -> Result<FindAllOutput, ErrorData> {
    let s = state.lock().await;
    let tab = current_tab(&s).await?;
    let els = resolve_all(&tab, &input.selector, input.limit).await?;
    let mut out = Vec::with_capacity(els.len());
    for el in &els {
        out.push(describe(el).await?);
    }
    Ok(FindAllOutput { elements: out })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            tag: None,
            attrs: vec![],
            nth: None,
            visible_only: true,
            timeout_ms: 5000,
            frame_id: None,
        }
    }

    #[tokio::test]
    async fn find_with_no_browser_suggests_browser_open() {
        let err = find(
            fresh(),
            FindInput {
                selector: css_sel("h1"),
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"), "msg: {}", err.message);
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[tokio::test]
    async fn find_all_with_no_browser_suggests_browser_open() {
        let err = find_all(
            fresh(),
            FindAllInput {
                selector: css_sel("a"),
                limit: 10,
            },
        )
        .await
        .expect_err("must error without an open browser");
        assert!(err.message.contains("browser_open"));
        let data = err.data.as_ref().expect("data populated");
        assert_eq!(data["suggested_next"], "browser_open");
    }

    #[test]
    fn compile_regex_rejects_malformed_pattern() {
        // `[unclosed` is a parse error in `regex`. The bridge surfaces it
        // as `invalid_params` so an agent can fix the pattern without
        // touching the live browser.
        let err = compile_regex("[unclosed").expect_err("malformed regex must error");
        assert!(
            err.message.contains("Invalid `text_regex`"),
            "msg: {}",
            err.message
        );
    }

    #[test]
    fn compile_regex_accepts_valid_pattern() {
        let re = compile_regex(r"^hello\s+world$").expect("valid regex");
        assert!(re.is_match("hello   world"));
    }

    #[test]
    fn parse_role_rejects_unknown_role_with_enum_list() {
        let err = parse_role("buttn").expect_err("typo must error");
        assert!(
            err.message.contains("Unknown ARIA role `buttn`"),
            "msg: {}",
            err.message
        );
        // The error must list valid alternatives so an agent can recover
        // without external docs.
        assert!(err.message.contains("button"));
        assert!(err.message.contains("textbox"));
    }

    #[test]
    fn parse_role_covers_every_lib_variant() {
        // Every variant from `AriaRole` (except `Other`) should be
        // routable. Other is intentionally not exposed over the wire.
        let cases = [
            ("button", AriaRole::Button),
            ("link", AriaRole::Link),
            ("textbox", AriaRole::Textbox),
            ("combobox", AriaRole::Combobox),
            ("checkbox", AriaRole::Checkbox),
            ("radio", AriaRole::Radio),
            ("tab", AriaRole::Tab),
            ("menu", AriaRole::Menu),
            ("menuitem", AriaRole::Menuitem),
            ("dialog", AriaRole::Dialog),
            ("heading", AriaRole::Heading),
            ("banner", AriaRole::Banner),
            ("navigation", AriaRole::Navigation),
            ("main", AriaRole::Main),
            ("article", AriaRole::Article),
            ("list", AriaRole::List),
            ("listitem", AriaRole::Listitem),
            ("row", AriaRole::Row),
            ("cell", AriaRole::Cell),
            ("columnheader", AriaRole::Columnheader),
            ("rowheader", AriaRole::Rowheader),
        ];
        for (s, expect) in cases {
            assert_eq!(parse_role(s).expect("ok"), expect, "input: {s}");
        }
    }
}
