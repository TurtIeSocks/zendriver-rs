//! Element query builders ([`FindBuilder`] / [`FindAllBuilder`]) and the
//! shared [`AriaRole`] / [`BoundingBox`] types.
//!
//! A query is a chainable selector + modifier + terminal sequence:
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! // Single match
//! let h1 = tab.find().css("h1").one().await?;
//! // First match by ARIA role + accessible name
//! use zendriver::AriaRole;
//! let btn = tab.find().role_named(AriaRole::Button, "Submit").one().await?;
//! // All matches
//! let links = tab.find_all().css("a").many_or_empty().await?;
//! # let _ = (h1, btn, links);
//! # Ok(()) }
//! ```
//!
//! Selector kinds: `.css`, `.xpath`, `.text`, `.text_exact`, `.text_regex`,
//! `.text_regex_with_flags`, `.role`, `.role_named`. Modifiers: `.nth`,
//! `.visible_only`, `.in_frame`, `.timeout`. Terminals on [`FindBuilder`]:
//! `.one()` (errors on empty), `.one_or_none()` (returns `None`). Terminals
//! on [`FindAllBuilder`]: `.many()` (errors on empty), `.many_or_empty()`
//! (returns empty `Vec`).
//!
//! # Predicate mode (bs4-like combinable matchers)
//!
//! Ten predicate methods let you match elements without writing a CSS
//! selector by hand. All active predicates are AND-ed together:
//!
//! | Method | Matches |
//! |---|---|
//! | `.tag(name)` | element tag name |
//! | `.attr(name, value)` | exact attribute value — `[name="value"]` |
//! | `.attr_contains(name, sub)` | attribute contains substring — `[name*="sub"]` |
//! | `.attr_starts_with(name, pre)` | attribute value prefix — `[name^="pre"]` |
//! | `.attr_ends_with(name, suf)` | attribute value suffix — `[name$="suf"]` |
//! | `.has_attr(name)` | attribute is present — `[name]` |
//! | `.attr_regex(name, pat)` | attribute value matches JS regex (post-filter) |
//! | `.containing_text(sub)` | element text contains substring (post-filter) |
//! | `.text_equals(exact)` | trimmed element text equals string (post-filter) |
//! | `.text_matches(pat)` | element text matches JS regex (post-filter) |
//!
//! Structural predicates (`.tag`, `.attr*`, `.has_attr`) compile to a single
//! CSS selector that the browser evaluates natively. Regex and text
//! predicates (`.attr_regex`, `.containing_text`, `.text_equals`,
//! `.text_matches`) are applied as a JS post-filter over the CSS candidates —
//! so the browser does the heavy lifting and only the survivors are inspected
//! in JS.
//!
//! **Mixing rule:** predicate methods cannot be combined with single-selector
//! methods (`.css()`, `.xpath()`, `.text*()`, `.role*()`). Using both on one
//! builder returns `Err(`[`crate::ZendriverError::ConflictingSelectors`]`)` at
//! the terminal (`.one()` / `.many()` / etc.).
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! // Predicate find: button whose class contains "primary" and text starts with "Buy"
//! let btn = tab
//!     .find()
//!     .tag("button")
//!     .attr_contains("class", "primary")
//!     .containing_text("Buy")
//!     .one()
//!     .await?;
//! // CSS convenience alias (equivalent to find().css(…).many())
//! let links = tab.select_all("nav a").await?;
//! # let _ = (btn, links);
//! # Ok(()) }
//! ```
//!
//! # Note: predicate elements are not auto-refreshable
//!
//! Elements found through predicate methods carry an *Evaluation* origin
//! (they are the result of a `Runtime.evaluate` / `callFunctionOn` call that
//! embeds the full predicate expression). Because the predicate set — especially
//! the regex and text post-filters — cannot be reduced to a single stored
//! selector string, the returned [`crate::Element`] handles **cannot be
//! transparently re-fetched** if the element is detached and the query needs
//! to be retried.
//!
//! Elements found via `.css()`, `.xpath()`, or the other single-selector
//! kinds are refreshable as before — they carry a stored selector that the
//! query layer can re-issue against the live DOM.

pub mod actionability;
pub mod modifiers;
pub(crate) mod predicate;
pub mod role;
pub mod selectors;

pub use role::AriaRole;

use std::time::Duration;

/// Element geometry in CSS pixels relative to the viewport's top-left.
///
/// Returned by [`crate::Element::bounding_box`] (and consumed by click
/// target calculations, screenshot clip rects, hover coordinates). Derived
/// from the `content` quad of `DOM.getBoxModel`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    /// Distance from viewport left edge to the box's left edge (CSS px).
    pub x: f64,
    /// Distance from viewport top edge to the box's top edge (CSS px).
    pub y: f64,
    /// Box width in CSS px.
    pub width: f64,
    /// Box height in CSS px.
    pub height: f64,
}

/// Page-absolute element geometry: a viewport-relative [`BoundingBox`] plus
/// the page scroll offset, so callers can compute coordinates relative to the
/// top-left of the *document* rather than the *viewport*.
///
/// Returned by [`crate::Element::bounding_box_page`]. Ports nodriver's
/// `Position.abs_x` / `abs_y` (element.py:504), which are the element
/// *center* offset by `window.scrollX` / `scrollY`. The viewport box and the
/// scroll offset are both retained so the page-absolute origin
/// ([`PageBox::abs_origin`]) and center ([`PageBox::abs_center`]) can both be
/// derived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageBox {
    /// The viewport-relative box (same as [`crate::Element::bounding_box`]).
    pub viewport: BoundingBox,
    /// `window.scrollX` at the time of measurement (CSS px the document is
    /// scrolled horizontally).
    pub scroll_x: f64,
    /// `window.scrollY` at the time of measurement (CSS px the document is
    /// scrolled vertically).
    pub scroll_y: f64,
}

impl PageBox {
    /// Page-absolute top-left of the box (`viewport.x + scroll_x`,
    /// `viewport.y + scroll_y`).
    ///
    /// This is nodriver's viewport `Position.x` / `.y` lifted into document
    /// coordinates.
    #[must_use]
    pub fn abs_origin(&self) -> (f64, f64) {
        (
            self.viewport.x + self.scroll_x,
            self.viewport.y + self.scroll_y,
        )
    }

    /// Page-absolute center of the box (origin + half the box size + scroll).
    ///
    /// Faithful to nodriver's `Position.abs_x` / `abs_y`, which are the
    /// element center offset by the scroll position.
    #[must_use]
    pub fn abs_center(&self) -> (f64, f64) {
        (
            self.viewport.x + self.viewport.width / 2.0 + self.scroll_x,
            self.viewport.y + self.viewport.height / 2.0 + self.scroll_y,
        )
    }
}

use tokio::time::Instant;

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::frame::Frame;
use crate::query::predicate::{AttrPred, PredicateSet, TextPred};
use crate::query::selectors::{QueryScope, RemoteRef, SelectorKind, text_len_of};
use crate::tab::Tab;

/// Generates the 10 predicate setters shared by `FindBuilder` + `FindAllBuilder`.
/// Both structs must have a `predicates: PredicateSet` field.
macro_rules! predicate_methods {
    () => {
        /// Match by tag name (compiles into the CSS selector). Predicate mode.
        #[must_use]
        pub fn tag(mut self, name: impl Into<String>) -> Self {
            self.predicates.tag = Some(name.into());
            self
        }
        /// Match an exact attribute value `[name="value"]`.
        #[must_use]
        pub fn attr(mut self, name: &str, value: &str) -> Self {
            self.predicates
                .attrs
                .push(AttrPred::Exact(name.into(), value.into()));
            self
        }
        /// Match a substring of an attribute value `[name*="sub"]`.
        #[must_use]
        pub fn attr_contains(mut self, name: &str, sub: &str) -> Self {
            self.predicates
                .attrs
                .push(AttrPred::Contains(name.into(), sub.into()));
            self
        }
        /// Match an attribute value prefix `[name^="pre"]`.
        #[must_use]
        pub fn attr_starts_with(mut self, name: &str, pre: &str) -> Self {
            self.predicates
                .attrs
                .push(AttrPred::StartsWith(name.into(), pre.into()));
            self
        }
        /// Match an attribute value suffix `[name$="suf"]`.
        #[must_use]
        pub fn attr_ends_with(mut self, name: &str, suf: &str) -> Self {
            self.predicates
                .attrs
                .push(AttrPred::EndsWith(name.into(), suf.into()));
            self
        }
        /// Require the attribute be present `[name]`.
        #[must_use]
        pub fn has_attr(mut self, name: &str) -> Self {
            self.predicates.attrs.push(AttrPred::Has(name.into()));
            self
        }
        /// Match an attribute value against a JS regex (post-filter).
        #[must_use]
        pub fn attr_regex(mut self, name: &str, pattern: &str) -> Self {
            self.predicates
                .attrs
                .push(AttrPred::Regex(name.into(), pattern.into()));
            self
        }
        /// Match elements whose text contains `sub` (post-filter).
        #[must_use]
        pub fn containing_text(mut self, sub: &str) -> Self {
            self.predicates.texts.push(TextPred::Contains(sub.into()));
            self
        }
        /// Match elements whose trimmed text equals `exact` (post-filter).
        #[must_use]
        pub fn text_equals(mut self, exact: &str) -> Self {
            self.predicates.texts.push(TextPred::Equals(exact.into()));
            self
        }
        /// Match elements whose text matches a JS regex `pattern` (post-filter).
        #[must_use]
        pub fn text_matches(mut self, pattern: &str) -> Self {
            self.predicates
                .texts
                .push(TextPred::Matches(pattern.into()));
            self
        }
    };
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// `true` when a query mixes a single-selector kind (`.css`/`.xpath`/
/// `.text*`/`.role*`) with bs4-like predicate methods (`.tag`/`.attr`/…).
/// The two styles are mutually exclusive; a terminal that sees a conflict
/// returns [`ZendriverError::ConflictingSelectors`] before dispatching any
/// CDP call. Extracted as a free function so the rule is unit-testable
/// without constructing a `Tab`.
fn has_conflict(selector: &Option<SelectorKind>, predicates: &PredicateSet) -> bool {
    selector.is_some() && !predicates.is_empty()
}

/// What a terminal resolves: either a single [`SelectorKind`] (with its
/// effective `best_match` flag) or a [`PredicateSet`]. Unifying the two
/// behind one type lets the poll loop, `nth`/`visible_only` pick, the
/// cross-frame fan-out, and `Element` synthesis be written ONCE — only
/// the per-scope candidate fetch and the synthesis provenance differ.
enum Resolver<'a> {
    /// Single-selector path. `best_match` is the already-resolved
    /// effective flag (`effective_best_match`), honored only for text
    /// selectors. Synthesizes elements with full `Query` provenance so
    /// `Element::refresh` can re-run the selector.
    Selector {
        selector: &'a SelectorKind,
        best_match: bool,
    },
    /// Predicate path. Synthesizes elements with `Evaluation` provenance
    /// (not auto-refreshable): the `Query` origin only carries a
    /// `SelectorKind`, and compiling a `PredicateSet` down to one would
    /// silently drop the regex/text post-filters on refresh — returning a
    /// *different* element. Losing refresh is the honest, correct choice
    /// over a refresh that re-resolves to the wrong node.
    Predicate { predicates: &'a PredicateSet },
}

impl Resolver<'_> {
    /// Resolve every candidate against `scope` in document order.
    async fn resolve(&self, scope: &QueryScope<'_>) -> Result<Vec<RemoteRef>> {
        match self {
            Resolver::Selector {
                selector,
                best_match,
            } => selector.resolve_many_inner(scope, *best_match).await,
            Resolver::Predicate { predicates } => {
                crate::query::selectors::resolve_predicate_many(scope, predicates).await
            }
        }
    }

    /// Wrap a resolved `RemoteRef` into an `Element`, preserving the
    /// provenance appropriate to this resolver (see the variant docs).
    fn synthesize(&self, r: RemoteRef, scope: &QueryScope<'_>, nth: usize) -> Element {
        match self {
            Resolver::Selector { selector, .. } => {
                Element::synthesize_query(r, scope, selector, nth)
            }
            Resolver::Predicate { .. } => Element::from_jsret(
                scope.synthesize_tab(),
                r.backend_node_id,
                r.remote_object_id,
            ),
        }
    }

    /// Short, log-friendly description for the `ElementNotFound` payload.
    fn describe(&self) -> String {
        match self {
            Resolver::Selector { selector, .. } => describe_selector(selector),
            Resolver::Predicate { predicates } => describe_predicates(predicates),
        }
    }

    /// `Some(selector)` only when this is a text selector with `best_match`
    /// active — the single case where the cross-frame fan-out runs the
    /// closest-text-length scoring path (`consider_scope_best` /
    /// `text_len_of`). Predicate and non-best-match selector resolvers
    /// return `None`, so they take the cheaper "first hit / concatenate"
    /// cross-frame branch.
    fn best_match_selector(&self) -> Option<&SelectorKind> {
        match self {
            Resolver::Selector {
                selector,
                best_match: true,
            } => Some(selector),
            _ => None,
        }
    }
}

/// Chainable single-element query builder.
///
/// Call a selector method, optionally chain modifiers, then terminate with
/// [`FindBuilder::one`] or [`FindBuilder::one_or_none`]. See the
/// [module-level docs](self) for the full surface map.
///
/// Selector kinds are mutually exclusive — calling `.css(...)` after
/// `.xpath(...)` overwrites the prior selector.
///
/// # Examples
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// # let browser = zendriver::Browser::builder().launch().await?;
/// # let tab = browser.main_tab();
/// let h1 = tab.find().css("h1").one().await?;
/// # let _ = h1;
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct FindBuilder<'scope> {
    /// Tab whose document is the default query root when neither
    /// [`Self::element`] nor [`Self::frame`] is set. Cleared (`None`)
    /// for `Frame`-rooted builders (built by
    /// [`FindBuilder::new_for_frame`]) since the parent tab is reachable
    /// via the frame's `Weak<TabInner>` only on Element synthesis, not
    /// on dispatch.
    pub(crate) tab: Option<&'scope Tab>,
    /// When set, the terminal resolves against this element's subtree
    /// (`QueryScope::Element`) rather than the whole tab (`QueryScope::Tab`).
    /// Populated by [`FindBuilder::new_for_element`]; the bare
    /// [`FindBuilder::new_for_tab`] leaves it `None`.
    pub(crate) element: Option<&'scope Element>,
    /// When set, the terminal resolves against this frame's document
    /// (`QueryScope::Frame`) — dispatching on the frame's own CDP
    /// session. Populated by [`FindBuilder::new_for_frame`]; mutually
    /// exclusive with `element` (a non-`None` value here also implies
    /// `tab = None`).
    pub(crate) frame: Option<&'scope Frame>,
    pub(crate) selector: Option<SelectorKind>,
    /// Accumulated bs4-like predicates. Mutually exclusive with `selector`
    /// (enforced at the terminal — see `one`/`one_or_none`).
    pub(crate) predicates: PredicateSet,
    pub(crate) timeout: Duration,
    pub(crate) nth: Option<usize>,
    pub(crate) visible_only: bool,
    /// Frame override populated by [`FindBuilder::in_frame`]. When set,
    /// the terminal swaps the scope to [`QueryScope::Frame`] before
    /// dispatch, routing commands through the frame's own session —
    /// even if the builder was originally rooted on a Tab. Element
    /// scope still takes precedence over both.
    pub(crate) in_frame: Option<&'scope Frame>,
    /// Opt-in cross-frame fan-out (default `false`). When `true` and the
    /// builder is Tab-scoped, the terminal resolves the main document
    /// **and** every [`Frame`] from [`Tab::frames`]. No-op for
    /// element/frame/`in_frame`-scoped builders. Set via
    /// [`FindBuilder::include_frames`].
    pub(crate) include_frames: bool,
    /// Opt-in closest-text-length matching (default `false`). When
    /// `true`, text selectors (`text`/`text_exact`/`text_regex`) return
    /// the candidate minimizing `abs(elementTextLen - needleLen)`; a
    /// no-op (+ debug log) on css/xpath/role. Set via
    /// [`FindBuilder::best_match`].
    pub(crate) best_match: bool,
}

impl<'scope> FindBuilder<'scope> {
    predicate_methods! {}

    pub(crate) fn new_for_tab(tab: &'scope Tab) -> Self {
        Self {
            tab: Some(tab),
            element: None,
            frame: None,
            selector: None,
            predicates: Default::default(),
            timeout: DEFAULT_TIMEOUT,
            nth: None,
            visible_only: false,
            in_frame: None,
            include_frames: false,
            best_match: false,
        }
    }

    /// Build a subtree-scoped query rooted at `element`. The terminal
    /// `one()` / `one_or_none()` resolves the selector against
    /// `element.querySelector(...)` (CSS) or the equivalent
    /// element-relative form for other selector kinds — matches outside
    /// the element's subtree are not considered.
    pub(crate) fn new_for_element(element: &'scope Element) -> Self {
        Self {
            tab: Some(element.tab()),
            element: Some(element),
            frame: None,
            selector: None,
            predicates: Default::default(),
            timeout: DEFAULT_TIMEOUT,
            nth: None,
            visible_only: false,
            in_frame: None,
            include_frames: false,
            best_match: false,
        }
    }

    /// Build a frame-scoped query rooted at `frame`. The terminal
    /// `one()` / `one_or_none()` resolves the selector against the
    /// frame's `document` and dispatches on the frame's own CDP
    /// session — distinct from the parent tab's session for OOPIFs
    /// (T16+), identical for same-origin sub-frames.
    pub(crate) fn new_for_frame(frame: &'scope Frame) -> Self {
        Self {
            tab: None,
            element: None,
            frame: Some(frame),
            selector: None,
            predicates: Default::default(),
            timeout: DEFAULT_TIMEOUT,
            nth: None,
            visible_only: false,
            in_frame: None,
            include_frames: false,
            best_match: false,
        }
    }

    // -- Selector methods (mutually exclusive — last call wins) --------

    /// CSS selector (e.g. `button.primary`, `[data-test=submit]`).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let btn = tab.find().css("button.primary").one().await?;
    /// # let _ = btn;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn css(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Css(selector.into()));
        self
    }

    /// XPath expression.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let h1 = tab.find().xpath("//h1").one().await?;
    /// # let _ = h1;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn xpath(mut self, expr: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Xpath(expr.into()));
        self
    }

    /// Case-insensitive substring text match.
    ///
    /// Walks the subtree filtering elements whose `innerText` (or
    /// `textContent` for hidden nodes) contains `needle` after lower-casing
    /// both sides.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let link = tab.find().text("more information").one().await?;
    /// # let _ = link;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn text(mut self, needle: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Text {
            needle: needle.into(),
            exact: false,
        });
        self
    }

    /// Whitespace-collapsed exact text match.
    ///
    /// Uses XPath `normalize-space(.)=<needle>`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let btn = tab.find().text_exact("Submit").one().await?;
    /// # let _ = btn;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn text_exact(mut self, needle: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Text {
            needle: needle.into(),
            exact: true,
        });
        self
    }

    /// Text regex match.
    ///
    /// The supplied [`regex::Regex`] is serialized to its pattern string and
    /// re-parsed on the JS side as `new RegExp(pattern, "")`. Use
    /// [`Self::text_regex_with_flags`] to pass explicit JS regex flags.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let re = regex::Regex::new(r"^Buy now").unwrap();
    /// let cta = tab.find().text_regex(re).one().await?;
    /// # let _ = cta;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn text_regex(mut self, re: regex::Regex) -> Self {
        self.selector = Some(SelectorKind::TextRegex {
            pattern: re.as_str().to_string(),
            flags: String::new(),
        });
        self
    }

    /// Text regex match with explicit JS-flavored flags.
    ///
    /// Flags follow JS RegExp syntax (e.g. `"i"` for case-insensitive,
    /// `"im"` for case-insensitive + multiline). The pattern is interpreted
    /// on the JS side via `new RegExp(pattern, flags)`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let el = tab.find().text_regex_with_flags(r"^accept", "i").one().await?;
    /// # let _ = el;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn text_regex_with_flags(
        mut self,
        pattern: impl Into<String>,
        flags: impl Into<String>,
    ) -> Self {
        self.selector = Some(SelectorKind::TextRegex {
            pattern: pattern.into(),
            flags: flags.into(),
        });
        self
    }

    /// ARIA role match.
    ///
    /// Compiles to a `[role="..."]` CSS attribute selector.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::AriaRole;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let btn = tab.find().role(AriaRole::Button).one().await?;
    /// # let _ = btn;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn role(mut self, role: AriaRole) -> Self {
        self.selector = Some(SelectorKind::Role(role, None));
        self
    }

    /// ARIA role + accessible name match.
    ///
    /// Post-filters role candidates by computed accessible name via
    /// `Accessibility.getPartialAXTree` (case-insensitive substring).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::AriaRole;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let submit = tab.find().role_named(AriaRole::Button, "Submit").one().await?;
    /// # let _ = submit;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn role_named(mut self, role: AriaRole, name: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Role(role, Some(name.into())));
        self
    }

    // -- Modifier methods ----------------------------------------------

    /// Pick the `idx`-th match (0-based) instead of the first.
    ///
    /// Combined with [`Self::visible_only`], the index applies AFTER the
    /// visibility filter.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let second_link = tab.find().css("a").nth(1).one().await?;
    /// # let _ = second_link;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn nth(mut self, idx: usize) -> Self {
        self.nth = Some(idx);
        self
    }

    /// Filter candidates by visibility before picking `nth`/first.
    ///
    /// When `true`, candidates that fail the visibility check (offscreen,
    /// `display: none`, etc.) are filtered out.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let visible_btn = tab.find().css("button").visible_only(true).one().await?;
    /// # let _ = visible_btn;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn visible_only(mut self, on: bool) -> Self {
        self.visible_only = on;
        self
    }

    /// Re-target this query at `frame`.
    ///
    /// The terminal dispatches on the frame's own CDP session. Element
    /// scope (if set via [`crate::Element::find`]) still takes precedence;
    /// this override applies only when the builder started from a Tab or
    /// another Frame.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let main = tab.main_frame().await?;
    /// let el = tab.find().in_frame(&main).css("h1").one().await?;
    /// # let _ = el;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn in_frame<'a>(self, frame: &'a Frame) -> FindBuilder<'a>
    where
        'scope: 'a,
    {
        FindBuilder {
            tab: self.tab,
            element: self.element,
            frame: self.frame,
            selector: self.selector,
            predicates: self.predicates,
            timeout: self.timeout,
            nth: self.nth,
            visible_only: self.visible_only,
            in_frame: Some(frame),
            include_frames: self.include_frames,
            best_match: self.best_match,
        }
    }

    /// Fan the query across the main document **and** every [`Frame`] in
    /// [`Tab::frames`] (each frame dispatches on its own CDP session).
    ///
    /// Opt-in (default off). `.one()` without [`Self::best_match`]
    /// resolves the main scope first and returns the first hit, only
    /// descending into frames on a miss (registry order, first frame hit
    /// wins). With `best_match` it gathers each scope's top candidate and
    /// picks the global closest-length winner. Only meaningful for a
    /// Tab-scoped builder — a no-op when the builder is element-, frame-,
    /// or [`Self::in_frame`]-scoped.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let cb = tab.find().text("verify you are human").include_frames().one().await?;
    /// # let _ = cb;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn include_frames(mut self) -> Self {
        self.include_frames = true;
        self
    }

    /// Among text-selector candidates, pick the one whose rendered text
    /// length is closest to the search needle (`abs(len(text) -
    /// len(needle))`).
    ///
    /// Opt-in (default off). Applies to [`Self::text`],
    /// [`Self::text_exact`], and [`Self::text_regex`] only — a no-op (with
    /// a `tracing::debug!` note) on css/xpath/role selectors, where text
    /// length is meaningless.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let btn = tab.find().text("accept all").best_match().one().await?;
    /// # let _ = btn;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn best_match(mut self) -> Self {
        self.best_match = true;
        self
    }

    /// Override the default 10s timeout for the terminal's poll loop.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let el = tab.find().css("#slow-load").timeout(Duration::from_secs(30)).one().await?;
    /// # let _ = el;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    // -- Terminals ------------------------------------------------------

    /// Wait for and return the first (or `nth`) matching element.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::ElementNotFound`] if no element matches
    /// within the timeout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let h1 = tab.find().css("h1").one().await?;
    /// # let _ = h1;
    /// # Ok(()) }
    /// ```
    pub async fn one(self) -> Result<Element> {
        // Mixing a single-selector kind with predicate methods is a usage
        // error — reject it before any CDP dispatch.
        if has_conflict(&self.selector, &self.predicates) {
            return Err(ZendriverError::ConflictingSelectors);
        }
        let predicate_mode = !self.predicates.is_empty();
        let selector = if predicate_mode {
            None
        } else {
            Some(self.selector.ok_or_else(|| {
                ZendriverError::Navigation(
                    "FindBuilder requires a selector (.css/.xpath/.text/.role/...) or a predicate (.tag/.attr/...)"
                        .into(),
                )
            })?)
        };
        // `best_match` only applies to text selectors; it is meaningless
        // (and unset) in predicate mode.
        let best_match = selector
            .as_ref()
            .is_some_and(|s| effective_best_match(self.best_match, s));
        // `selector` is `Some` iff we are *not* in predicate mode (the
        // `else` arm above either bound it or returned early), so the
        // resolver follows directly from whether a selector is present.
        let resolver = match &selector {
            Some(sel) => Resolver::Selector {
                selector: sel,
                best_match,
            },
            None => Resolver::Predicate {
                predicates: &self.predicates,
            },
        };

        let deadline = Instant::now() + self.timeout;
        let want_nth = self.nth.unwrap_or(0);

        // `include_frames` only fans out for a plain Tab-scoped builder.
        // Element/in_frame/Frame scopes are intentionally narrow, so the
        // modifier is a documented no-op there.
        let fan_frames = self.include_frames
            && self.element.is_none()
            && self.in_frame.is_none()
            && self.frame.is_none();

        if let (true, Some(tab)) = (fan_frames, self.tab) {
            return one_across_frames(tab, &resolver, want_nth, deadline).await;
        }

        // Precedence: Element > in_frame override > Frame > Tab.
        // - Element-scoped queries always win (they were built via
        //   `Element::find` and constrain to that subtree explicitly).
        // - `in_frame(&Frame)` overrides a co-stored Tab default so
        //   `tab.find().in_frame(&f).css(...)` dispatches on the
        //   Frame's session.
        // - Frame-scoped queries (built via `Frame::find`) keep their
        //   stored frame ref when no override is set.
        let scope = match (self.element, self.in_frame, self.frame, self.tab) {
            (Some(el), _, _, _) => QueryScope::Element(el),
            (None, Some(fr), _, _) => QueryScope::Frame(fr),
            (None, None, Some(fr), _) => QueryScope::Frame(fr),
            (None, None, None, Some(tab)) => QueryScope::Tab(tab),
            (None, None, None, None) => {
                return Err(ZendriverError::Navigation(
                    "FindBuilder has no scope (no tab, element, or frame)".into(),
                ));
            }
        };
        loop {
            let candidates = resolver.resolve(&scope).await?;

            // Visible-only filter: TODO(T16) — depends on
            // `actionability::check_visible`, which depends on
            // `Element::call_on_main`. Until that lands, treat every
            // candidate as visible so the wider FindBuilder API can ship.
            let _ = self.visible_only;
            let filtered = candidates;

            if let Some(picked) = filtered.into_iter().nth(want_nth) {
                return Ok(resolver.synthesize(picked, &scope, want_nth));
            }
            if Instant::now() >= deadline {
                return Err(ZendriverError::ElementNotFound {
                    selector: resolver.describe(),
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Like [`Self::one`], but returns `None` instead of erroring when no
    /// element matches within the timeout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// if let Some(banner) = tab.find().css(".cookie-banner").one_or_none().await? {
    ///     banner.find().css("button").one().await?.click().await?;
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn one_or_none(self) -> Result<Option<Element>> {
        match self.one().await {
            Ok(el) => Ok(Some(el)),
            Err(ZendriverError::ElementNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Chainable element query returning ALL matches.
///
/// Mirrors [`FindBuilder`] selectors + modifiers, minus `nth` (which
/// doesn't make sense for a "return everything" terminal). Selector kinds
/// are mutually exclusive — calling `.css(...)` after `.xpath(...)`
/// overwrites the prior selector.
///
/// Terminals: [`FindAllBuilder::many`] errors when the result is empty;
/// [`FindAllBuilder::many_or_empty`] returns an empty `Vec` instead.
///
/// # Examples
///
/// ```no_run
/// # async fn ex() -> zendriver::Result<()> {
/// # let browser = zendriver::Browser::builder().launch().await?;
/// # let tab = browser.main_tab();
/// let links = tab.find_all().css("a").many_or_empty().await?;
/// println!("{} links", links.len());
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct FindAllBuilder<'scope> {
    /// Tab whose document is the default query root. `None` when the
    /// builder is rooted at a `Frame` instead (see [`Self::frame`]).
    pub(crate) tab: Option<&'scope Tab>,
    /// When set, the terminal resolves against this element's subtree
    /// (`QueryScope::Element`) rather than the whole tab (`QueryScope::Tab`).
    /// Populated by [`FindAllBuilder::new_for_element`]; the bare
    /// [`FindAllBuilder::new_for_tab`] leaves it `None`.
    pub(crate) element: Option<&'scope Element>,
    /// When set, the terminal resolves against this frame's document
    /// and dispatches on the frame's own CDP session. Populated by
    /// [`FindAllBuilder::new_for_frame`].
    pub(crate) frame: Option<&'scope Frame>,
    pub(crate) selector: Option<SelectorKind>,
    /// Accumulated bs4-like predicates. Mutually exclusive with `selector`
    /// (enforced at the terminal — see `many`/`many_or_empty`).
    pub(crate) predicates: PredicateSet,
    pub(crate) timeout: Duration,
    pub(crate) visible_only: bool,
    /// Frame override populated by [`FindAllBuilder::in_frame`]. See
    /// the corresponding field on [`FindBuilder`] for the precedence
    /// rationale.
    pub(crate) in_frame: Option<&'scope Frame>,
    /// Opt-in cross-frame fan-out (default `false`). See
    /// [`FindBuilder::include_frames`]; for `many()` the results are
    /// concatenated main-first then per-frame in registry order.
    pub(crate) include_frames: bool,
    /// Opt-in closest-text-length ordering (default `false`). See
    /// [`FindBuilder::best_match`]; for `many()` it orders the returned
    /// Vec closest-length first within each scope.
    pub(crate) best_match: bool,
}

impl<'scope> FindAllBuilder<'scope> {
    predicate_methods! {}

    pub(crate) fn new_for_tab(tab: &'scope Tab) -> Self {
        Self {
            tab: Some(tab),
            element: None,
            frame: None,
            selector: None,
            predicates: Default::default(),
            timeout: DEFAULT_TIMEOUT,
            visible_only: false,
            in_frame: None,
            include_frames: false,
            best_match: false,
        }
    }

    /// Build a subtree-scoped `find_all` rooted at `element`. The
    /// terminal `many()` / `many_or_empty()` resolves the selector
    /// against the element's subtree — siblings and ancestors are not
    /// considered.
    pub(crate) fn new_for_element(element: &'scope Element) -> Self {
        Self {
            tab: Some(element.tab()),
            element: Some(element),
            frame: None,
            selector: None,
            predicates: Default::default(),
            timeout: DEFAULT_TIMEOUT,
            visible_only: false,
            in_frame: None,
            include_frames: false,
            best_match: false,
        }
    }

    /// Build a frame-scoped `find_all` rooted at `frame`. The terminal
    /// `many()` / `many_or_empty()` resolves the selector against the
    /// frame's `document` and dispatches on the frame's own CDP session.
    pub(crate) fn new_for_frame(frame: &'scope Frame) -> Self {
        Self {
            tab: None,
            element: None,
            frame: Some(frame),
            selector: None,
            predicates: Default::default(),
            timeout: DEFAULT_TIMEOUT,
            visible_only: false,
            in_frame: None,
            include_frames: false,
            best_match: false,
        }
    }

    // -- Selector methods (mutually exclusive — last call wins) --------

    /// CSS selector. See [`FindBuilder::css`].
    #[must_use]
    pub fn css(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Css(selector.into()));
        self
    }

    /// XPath expression. See [`FindBuilder::xpath`].
    #[must_use]
    pub fn xpath(mut self, expr: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Xpath(expr.into()));
        self
    }

    /// Case-insensitive substring text match. See [`FindBuilder::text`].
    #[must_use]
    pub fn text(mut self, needle: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Text {
            needle: needle.into(),
            exact: false,
        });
        self
    }

    /// Whitespace-collapsed exact text match. See [`FindBuilder::text_exact`].
    #[must_use]
    pub fn text_exact(mut self, needle: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Text {
            needle: needle.into(),
            exact: true,
        });
        self
    }

    /// Text regex match. See [`FindBuilder::text_regex`].
    #[must_use]
    pub fn text_regex(mut self, re: regex::Regex) -> Self {
        self.selector = Some(SelectorKind::TextRegex {
            pattern: re.as_str().to_string(),
            flags: String::new(),
        });
        self
    }

    /// Text regex match with explicit JS-flavored flags.
    /// See [`FindBuilder::text_regex_with_flags`].
    #[must_use]
    pub fn text_regex_with_flags(
        mut self,
        pattern: impl Into<String>,
        flags: impl Into<String>,
    ) -> Self {
        self.selector = Some(SelectorKind::TextRegex {
            pattern: pattern.into(),
            flags: flags.into(),
        });
        self
    }

    /// ARIA role match. See [`FindBuilder::role`].
    #[must_use]
    pub fn role(mut self, role: AriaRole) -> Self {
        self.selector = Some(SelectorKind::Role(role, None));
        self
    }

    /// ARIA role + accessible name match. See [`FindBuilder::role_named`].
    #[must_use]
    pub fn role_named(mut self, role: AriaRole, name: impl Into<String>) -> Self {
        self.selector = Some(SelectorKind::Role(role, Some(name.into())));
        self
    }

    // -- Modifier methods ----------------------------------------------

    /// Filter candidates by visibility before returning.
    /// See [`FindBuilder::visible_only`].
    #[must_use]
    pub fn visible_only(mut self, on: bool) -> Self {
        self.visible_only = on;
        self
    }

    /// Re-target this query at `frame`. See [`FindBuilder::in_frame`].
    #[must_use]
    pub fn in_frame<'a>(self, frame: &'a Frame) -> FindAllBuilder<'a>
    where
        'scope: 'a,
    {
        FindAllBuilder {
            tab: self.tab,
            element: self.element,
            frame: self.frame,
            selector: self.selector,
            predicates: self.predicates,
            timeout: self.timeout,
            visible_only: self.visible_only,
            in_frame: Some(frame),
            include_frames: self.include_frames,
            best_match: self.best_match,
        }
    }

    /// Fan the query across the main document and every [`Frame`].
    /// See [`FindBuilder::include_frames`]. For `many()` the matches are
    /// concatenated main-first then per-frame in registry order. No-op
    /// for element/frame/`in_frame`-scoped builders.
    #[must_use]
    pub fn include_frames(mut self) -> Self {
        self.include_frames = true;
        self
    }

    /// Order text-selector matches closest-text-length first.
    /// See [`FindBuilder::best_match`]. A no-op (+ debug log) on
    /// css/xpath/role.
    #[must_use]
    pub fn best_match(mut self) -> Self {
        self.best_match = true;
        self
    }

    /// Override the default 10s timeout for the poll loop.
    ///
    /// The loop returns the first non-empty result it observes; on timeout
    /// `many()` errors with `ElementNotFound` and `many_or_empty()` returns
    /// an empty `Vec`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let items = tab.find_all().css(".item").timeout(Duration::from_secs(20)).many().await?;
    /// # let _ = items;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    // -- Terminals ------------------------------------------------------

    /// Wait for and return ALL matching elements.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::ElementNotFound`] if no element matches
    /// within the timeout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let cells = tab.find_all().css("table td").many().await?;
    /// println!("{} cells", cells.len());
    /// # Ok(()) }
    /// ```
    pub async fn many(self) -> Result<Vec<Element>> {
        // Reject mixing single-selector + predicate methods before any
        // CDP dispatch (see `FindBuilder::one`).
        if has_conflict(&self.selector, &self.predicates) {
            return Err(ZendriverError::ConflictingSelectors);
        }
        let predicate_mode = !self.predicates.is_empty();
        let selector = if predicate_mode {
            None
        } else {
            Some(self.selector.ok_or_else(|| {
                ZendriverError::Navigation(
                    "FindAllBuilder requires a selector (.css/.xpath/.text/.role/...) or a predicate (.tag/.attr/...)"
                        .into(),
                )
            })?)
        };
        let best_match = selector
            .as_ref()
            .is_some_and(|s| effective_best_match(self.best_match, s));
        // See `FindBuilder::one`: `selector` is `Some` iff not in predicate
        // mode, so the resolver follows directly from its presence.
        let resolver = match &selector {
            Some(sel) => Resolver::Selector {
                selector: sel,
                best_match,
            },
            None => Resolver::Predicate {
                predicates: &self.predicates,
            },
        };

        let deadline = Instant::now() + self.timeout;

        let fan_frames = self.include_frames
            && self.element.is_none()
            && self.in_frame.is_none()
            && self.frame.is_none();

        if let (true, Some(tab)) = (fan_frames, self.tab) {
            return many_across_frames(tab, &resolver, deadline).await;
        }

        // See `FindBuilder::one` for the precedence rationale.
        let scope = match (self.element, self.in_frame, self.frame, self.tab) {
            (Some(el), _, _, _) => QueryScope::Element(el),
            (None, Some(fr), _, _) => QueryScope::Frame(fr),
            (None, None, Some(fr), _) => QueryScope::Frame(fr),
            (None, None, None, Some(tab)) => QueryScope::Tab(tab),
            (None, None, None, None) => {
                return Err(ZendriverError::Navigation(
                    "FindAllBuilder has no scope (no tab, element, or frame)".into(),
                ));
            }
        };
        loop {
            let candidates = resolver.resolve(&scope).await?;

            // Visible-only filter: TODO(T16) — depends on
            // `actionability::check_visible`, which depends on
            // `Element::call_on_main`. Until that lands, treat every
            // candidate as visible so the wider FindAllBuilder API can
            // ship.
            let _ = self.visible_only;
            let filtered = candidates;

            if !filtered.is_empty() {
                let elements: Vec<Element> = filtered
                    .into_iter()
                    .enumerate()
                    .map(|(i, r)| resolver.synthesize(r, &scope, i))
                    .collect();
                return Ok(elements);
            }
            if Instant::now() >= deadline {
                return Err(ZendriverError::ElementNotFound {
                    selector: resolver.describe(),
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Like [`Self::many`], but returns an empty `Vec` instead of erroring
    /// when no element matches within the timeout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let warnings = tab.find_all().css(".warning").many_or_empty().await?;
    /// for w in warnings {
    ///     eprintln!("{}", w.inner_text().await?);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn many_or_empty(self) -> Result<Vec<Element>> {
        match self.many().await {
            Ok(els) => Ok(els),
            Err(ZendriverError::ElementNotFound { .. }) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }
}

/// `true` for text selectors (`text` / `text_exact` / `text_regex`),
/// the only kinds for which `best_match` (closest-text-length scoring)
/// is meaningful.
fn selector_is_text(sel: &SelectorKind) -> bool {
    matches!(
        sel,
        SelectorKind::Text { .. } | SelectorKind::TextRegex { .. }
    )
}

/// Length (in `char`s) of the needle a text selector compares against —
/// the literal needle for `Text`, or the pattern string as the only
/// well-defined proxy for `TextRegex`. `None` for non-text selectors
/// (which never reach the `best_match` cross-scope path). Mirrors the
/// per-scope JS sort key so cross-scope scoring is consistent with the
/// in-scope ordering.
fn needle_len_of(sel: &SelectorKind) -> Option<usize> {
    match sel {
        SelectorKind::Text { needle, .. } => Some(needle.chars().count()),
        SelectorKind::TextRegex { pattern, .. } => Some(pattern.chars().count()),
        _ => None,
    }
}

/// Resolve the caller's `best_match` request against the selector kind:
/// honored for text selectors, ignored (with a `tracing::debug!` note)
/// for css/xpath/role where text length is meaningless.
fn effective_best_match(requested: bool, sel: &SelectorKind) -> bool {
    if requested && !selector_is_text(sel) {
        tracing::debug!("best_match ignored for non-text selector");
        return false;
    }
    requested && selector_is_text(sel)
}

/// One poll of a cross-frame `.one()`: resolve the main document scope
/// first, then descend into each [`Frame`] from [`Tab::frames`].
///
/// Without `best_match`, the first hit wins (main, then frames in
/// registry order) — early-return keeps the common case cheap. With
/// `best_match`, every scope's top candidate is read for its text length
/// (one extra `Runtime.callFunctionOn` per scope) and the global
/// closest-length winner is returned; ties resolve to the earliest scope
/// (main before frames, frames in registry order).
async fn one_across_frames(
    tab: &Tab,
    resolver: &Resolver<'_>,
    want_nth: usize,
    deadline: Instant,
) -> Result<Element> {
    loop {
        let frames = tab.frames().await?;

        if let Some(selector) = resolver.best_match_selector() {
            // Gather each scope's nth candidate + its text-length distance
            // to the needle, then pick the global minimum. The per-scope
            // JS sort already puts the closest-length candidate at index
            // `want_nth`; we re-read its raw length here and recompute the
            // distance to compare *across* scopes. Only text selectors
            // with `best_match` reach here (see `best_match_selector`).
            let needle_len = needle_len_of(selector).unwrap_or(0);
            let mut best: Option<(usize, RemoteRef, ScopeTag)> = None;
            let main_scope = QueryScope::Tab(tab);
            consider_scope_best(
                &main_scope,
                selector,
                want_nth,
                needle_len,
                ScopeTag::Main,
                &mut best,
            )
            .await?;
            for (i, frame) in frames.iter().enumerate() {
                let scope = QueryScope::Frame(frame);
                consider_scope_best(
                    &scope,
                    selector,
                    want_nth,
                    needle_len,
                    ScopeTag::Frame(i),
                    &mut best,
                )
                .await?;
            }
            if let Some((_, picked, tag)) = best {
                let scope = match tag {
                    ScopeTag::Main => QueryScope::Tab(tab),
                    ScopeTag::Frame(i) => QueryScope::Frame(&frames[i]),
                };
                return Ok(Element::synthesize_query(
                    picked, &scope, selector, want_nth,
                ));
            }
        } else {
            // First hit wins: main first, then frames in registry order.
            // Covers both plain selectors and the predicate path (which
            // never uses best_match). `resolver.resolve` dispatches the
            // per-scope query — predicate or selector — against each scope.
            let main_scope = QueryScope::Tab(tab);
            let main_hits = resolver.resolve(&main_scope).await?;
            if let Some(picked) = main_hits.into_iter().nth(want_nth) {
                return Ok(resolver.synthesize(picked, &main_scope, want_nth));
            }
            for frame in &frames {
                let scope = QueryScope::Frame(frame);
                let hits = resolver.resolve(&scope).await?;
                if let Some(picked) = hits.into_iter().nth(want_nth) {
                    return Ok(resolver.synthesize(picked, &scope, want_nth));
                }
            }
        }

        if Instant::now() >= deadline {
            return Err(ZendriverError::ElementNotFound {
                selector: resolver.describe(),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One poll of a cross-frame `.many()`: gather matches from the main
/// document **and** every [`Frame`], concatenated main-first then
/// per-frame in registry order. Polls until the aggregate is non-empty
/// or the deadline passes (mirroring the single-scope `many()` loop).
async fn many_across_frames(
    tab: &Tab,
    resolver: &Resolver<'_>,
    deadline: Instant,
) -> Result<Vec<Element>> {
    loop {
        let frames = tab.frames().await?;
        let mut elements: Vec<Element> = Vec::new();

        let main_scope = QueryScope::Tab(tab);
        for (i, r) in resolver.resolve(&main_scope).await?.into_iter().enumerate() {
            elements.push(resolver.synthesize(r, &main_scope, i));
        }
        for frame in &frames {
            let scope = QueryScope::Frame(frame);
            for (i, r) in resolver.resolve(&scope).await?.into_iter().enumerate() {
                elements.push(resolver.synthesize(r, &scope, i));
            }
        }

        if !elements.is_empty() {
            return Ok(elements);
        }
        if Instant::now() >= deadline {
            return Err(ZendriverError::ElementNotFound {
                selector: resolver.describe(),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Identifies which scope a cross-frame candidate came from so ties
/// resolve deterministically (main before any frame; frames by registry
/// index).
#[derive(Clone, Copy)]
enum ScopeTag {
    Main,
    Frame(usize),
}

/// Resolve `selector` against `scope`, take its `nth` candidate, read
/// that candidate's text-length distance to `needle_len`, and update
/// `best` if this scope beats the current global minimum. Strictly-less
/// comparison keeps ties on the earliest scope (callers invoke main
/// first, then frames in order). The stored key is `abs(len -
/// needle_len)`, matching the per-scope JS sort.
async fn consider_scope_best(
    scope: &QueryScope<'_>,
    selector: &SelectorKind,
    want_nth: usize,
    needle_len: usize,
    tag: ScopeTag,
    best: &mut Option<(usize, RemoteRef, ScopeTag)>,
) -> Result<()> {
    let hits = selector.resolve_many_inner(scope, true).await?;
    let Some(candidate) = hits.into_iter().nth(want_nth) else {
        return Ok(());
    };
    let len = text_len_of(scope, &candidate).await?;
    let dist = len.abs_diff(needle_len);
    if best
        .as_ref()
        .is_none_or(|(best_dist, _, _)| dist < *best_dist)
    {
        *best = Some((dist, candidate, tag));
    }
    Ok(())
}

/// Render a short, log-friendly description of a `SelectorKind` so the
/// `ElementNotFound { selector }` payload conveys what the user asked
/// for, regardless of which selector kind they chose.
fn describe_selector(sel: &SelectorKind) -> String {
    match sel {
        SelectorKind::Css(s) => format!("css({s})"),
        SelectorKind::Xpath(s) => format!("xpath({s})"),
        SelectorKind::Text { needle, exact } => {
            if *exact {
                format!("text_exact({needle})")
            } else {
                format!("text({needle})")
            }
        }
        SelectorKind::TextRegex { pattern, flags } => {
            if flags.is_empty() {
                format!("text_regex(/{pattern}/)")
            } else {
                format!("text_regex(/{pattern}/{flags})")
            }
        }
        SelectorKind::Role(role, None) => format!("role({})", role.to_css()),
        SelectorKind::Role(role, Some(name)) => {
            format!("role_named({}, {name})", role.to_css())
        }
    }
}

/// Render a short, log-friendly description of a [`PredicateSet`] for the
/// `ElementNotFound { selector }` payload — the compiled CSS selector,
/// plus the JS post-filter when any `attr_regex`/text predicate is set.
fn describe_predicates(pred: &PredicateSet) -> String {
    let css = pred.to_css_selector();
    let filter = pred.to_js_filter();
    if filter == "true" {
        format!("predicate(css={css})")
    } else {
        format!("predicate(css={css}, filter={filter})")
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    #[tokio::test]
    async fn one_returns_element_when_query_selector_matches() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.find().css("#b").one().await }
        });

        // T12: one() now resolves via SelectorKind::resolve_many, which
        // for CSS dispatches `Array.from(document.querySelectorAll(...))`.
        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains("document.querySelectorAll") && sent.contains("#b"),
            "expected querySelectorAll with selector, got: {sent}"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        // Enumerate the array — one element at index 0.
        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RArr");
        mock.reply(
            id_p,
            json!({
                "result": [
                    {
                        "name": "0",
                        "value": { "objectId": "R1", "type": "object", "subtype": "node" }
                    },
                    {
                        "name": "length",
                        "value": { "value": 1, "type": "number" }
                    }
                ]
            }),
        )
        .await;

        // describe the picked node to fill backend_node_id.
        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R1");
        mock.reply(id_d, json!({ "node": { "backendNodeId": 42 } }))
            .await;

        let el = fut.await.unwrap().unwrap();
        assert_eq!(*el.inner.backend_node_id.lock().await, Some(42));
        assert_eq!(
            el.inner.remote_object_id.lock().await.as_deref(),
            Some("R1")
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn one_returns_element_not_found_when_query_returns_empty() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.find()
                    .css("#missing")
                    .timeout(Duration::from_millis(150))
                    .one()
                    .await
            }
        });

        // The builder polls until timeout. Each poll: Runtime.evaluate
        // (returning an empty array RemoteObject) → Runtime.getProperties
        // (zero indexed entries). Respond to both each iteration so
        // resolve_many returns an empty Vec rather than erroring.
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(220)) => break,
                cmd = mock.expect_cmd("Runtime.evaluate") => {
                    mock.reply(
                        cmd,
                        json!({ "result": { "objectId": "RArrEmpty", "type": "object", "subtype": "array" } }),
                    )
                    .await;
                    let id_p = mock.expect_cmd("Runtime.getProperties").await;
                    mock.reply(id_p, json!({ "result": [
                        { "name": "length", "value": { "value": 0, "type": "number" } }
                    ] })).await;
                }
            }
        }

        let res = fut.await.unwrap();
        match res {
            Err(ZendriverError::ElementNotFound { selector }) => {
                assert!(selector.contains("#missing"), "got: {selector}");
            }
            Err(e) => panic!("unexpected error: {e:?}"),
            Ok(_) => panic!("unexpected ok"),
        }
        conn.shutdown();
    }

    #[tokio::test]
    async fn one_or_none_returns_none_on_timeout() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.find()
                    .css("#missing")
                    .timeout(Duration::from_millis(120))
                    .one_or_none()
                    .await
            }
        });

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => break,
                cmd = mock.expect_cmd("Runtime.evaluate") => {
                    mock.reply(
                        cmd,
                        json!({ "result": { "objectId": "RArrEmpty", "type": "object", "subtype": "array" } }),
                    )
                    .await;
                    let id_p = mock.expect_cmd("Runtime.getProperties").await;
                    mock.reply(id_p, json!({ "result": [
                        { "name": "length", "value": { "value": 0, "type": "number" } }
                    ] })).await;
                }
            }
        }

        let res = fut.await.unwrap().unwrap();
        assert!(res.is_none());
        conn.shutdown();
    }

    // --- describe_selector renders each kind --------------------------

    #[test]
    fn describe_selector_renders_each_kind() {
        assert_eq!(
            describe_selector(&SelectorKind::Css("#b".into())),
            "css(#b)"
        );
        assert_eq!(
            describe_selector(&SelectorKind::Xpath("//a".into())),
            "xpath(//a)"
        );
        assert_eq!(
            describe_selector(&SelectorKind::Text {
                needle: "Hi".into(),
                exact: false,
            }),
            "text(Hi)"
        );
        assert_eq!(
            describe_selector(&SelectorKind::Text {
                needle: "Hi".into(),
                exact: true,
            }),
            "text_exact(Hi)"
        );
        assert_eq!(
            describe_selector(&SelectorKind::TextRegex {
                pattern: "a.*b".into(),
                flags: String::new(),
            }),
            "text_regex(/a.*b/)"
        );
        assert_eq!(
            describe_selector(&SelectorKind::TextRegex {
                pattern: "a.*b".into(),
                flags: "i".into(),
            }),
            "text_regex(/a.*b/i)"
        );
        assert_eq!(
            describe_selector(&SelectorKind::Role(AriaRole::Button, None)),
            r#"role([role="button"])"#
        );
        assert_eq!(
            describe_selector(&SelectorKind::Role(AriaRole::Button, Some("Save".into()))),
            r#"role_named([role="button"], Save)"#
        );
    }

    // --- selector kind chaining: last call wins -----------------------

    #[tokio::test]
    async fn text_regex_wraps_regex_pattern_and_empty_flags() {
        let re = regex::Regex::new("hello.*world").unwrap();
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let fb = tab.find().text_regex(re);
        let Some(SelectorKind::TextRegex { pattern, flags }) = fb.selector else {
            panic!("expected TextRegex selector kind");
        };
        assert_eq!(pattern, "hello.*world");
        assert_eq!(flags, "");
        conn.shutdown();
    }

    #[tokio::test]
    async fn text_regex_with_flags_passes_pattern_and_flags_through() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let fb = tab.find().text_regex_with_flags("h.*w", "im");
        let Some(SelectorKind::TextRegex { pattern, flags }) = fb.selector else {
            panic!("expected TextRegex selector kind");
        };
        assert_eq!(pattern, "h.*w");
        assert_eq!(flags, "im");
        conn.shutdown();
    }

    #[tokio::test]
    async fn later_selector_overrides_earlier() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);
        let fb = tab.find().css("#a").xpath("//b");
        let Some(SelectorKind::Xpath(expr)) = fb.selector else {
            panic!("expected Xpath selector kind after .xpath() override");
        };
        assert_eq!(expr, "//b");
        conn.shutdown();
    }

    #[tokio::test]
    async fn modifiers_chain_and_persist_on_builder() {
        let (_mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess.clone());
        let frame = Frame::new(
            "F2".into(),
            None,
            String::new(),
            None,
            sess,
            std::sync::Weak::new(),
        );
        let fb = tab
            .find()
            .css(".item")
            .nth(3)
            .visible_only(true)
            .in_frame(&frame)
            .timeout(Duration::from_secs(5));
        assert_eq!(fb.nth, Some(3));
        assert!(fb.visible_only);
        assert!(fb.in_frame.is_some(), "in_frame must hold Frame ref");
        assert_eq!(fb.in_frame.unwrap().id(), "F2");
        assert_eq!(fb.timeout, Duration::from_secs(5));
        conn.shutdown();
    }

    /// T17: `tab.find().in_frame(&frame).css(...).one()` must dispatch
    /// `Runtime.evaluate` on the Frame's session, NOT the Tab's. The
    /// override flips the scope from `QueryScope::Tab` to
    /// `QueryScope::Frame` so the frame's CDP session routes the
    /// command — load-bearing for OOPIFs where the two sessions are
    /// distinct.
    #[tokio::test]
    async fn in_frame_override_routes_dispatch_to_frame_session() {
        let (mut mock, conn) = MockConnection::pair();
        // Two distinct sessions over the same mock connection so we can
        // tell which one dispatched the command via the `sessionId`
        // field on the outbound frame.
        let tab_sess = SessionHandle::new(conn.clone(), "S_TAB");
        let frame_sess = SessionHandle::new(conn.clone(), "S_FRAME");
        let tab = Tab::new_for_test(tab_sess);
        let frame = Frame::new(
            "F_OOPIF".into(),
            None,
            String::new(),
            None,
            frame_sess,
            std::sync::Arc::downgrade(&tab.inner),
        );

        let fut = tokio::spawn({
            let t = tab.clone();
            let f = frame.clone();
            async move { t.find().in_frame(&f).css("button").one().await }
        });

        // Frame scope first allocates an isolated world on the Frame's
        // session so the follow-up `Runtime.evaluate` runs against the
        // frame's document rather than the parent tab's default
        // context. The override under test routes the createIsolatedWorld
        // call to S_FRAME just like every subsequent dispatch.
        let id_iso = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(
            mock.last_sent()["sessionId"],
            "S_FRAME",
            "in_frame override must route Page.createIsolatedWorld through the Frame's session"
        );
        assert_eq!(mock.last_sent()["params"]["frameId"], "F_OOPIF");
        mock.reply(id_iso, json!({ "executionContextId": 9001 }))
            .await;

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(
            mock.last_sent()["sessionId"],
            "S_FRAME",
            "in_frame override must route Runtime.evaluate through the Frame's session, not the Tab's"
        );
        assert_eq!(
            mock.last_sent()["params"]["contextId"],
            9001,
            "Runtime.evaluate must be pinned to the frame's isolated-world contextId"
        );
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains("document.querySelectorAll") && sent.contains("button"),
            "expected querySelectorAll with selector, got: {sent}"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        assert_eq!(
            mock.last_sent()["sessionId"],
            "S_FRAME",
            "follow-up Runtime.getProperties must also dispatch on the Frame's session"
        );
        mock.reply(
            id_p,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "R0", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 1, "type": "number" } }
                ]
            }),
        )
        .await;

        let id_d = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(
            mock.last_sent()["sessionId"],
            "S_FRAME",
            "DOM.describeNode must also dispatch on the Frame's session"
        );
        mock.reply(id_d, json!({ "node": { "backendNodeId": 7 } }))
            .await;

        let el = fut.await.unwrap().unwrap();
        assert_eq!(*el.inner.backend_node_id.lock().await, Some(7));
        conn.shutdown();
    }

    #[tokio::test]
    async fn many_returns_all_matches() {
        // Two-match scenario: many() must return BOTH elements as a Vec
        // (vs FindBuilder::one which picks one based on nth).
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.find_all().css(".item").many().await }
        });

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains("document.querySelectorAll") && sent.contains(".item"),
            "expected querySelectorAll with selector, got: {sent}"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RArr");
        mock.reply(
            id_p,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "R0", "type": "object", "subtype": "node" } },
                    { "name": "1", "value": { "objectId": "R1", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 2, "type": "number" } }
                ]
            }),
        )
        .await;

        let id_d0 = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R0");
        mock.reply(id_d0, json!({ "node": { "backendNodeId": 20 } }))
            .await;
        let id_d1 = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R1");
        mock.reply(id_d1, json!({ "node": { "backendNodeId": 21 } }))
            .await;

        let els = fut.await.unwrap().unwrap();
        assert_eq!(els.len(), 2);
        assert_eq!(
            els[0].inner.remote_object_id.lock().await.as_deref(),
            Some("R0")
        );
        assert_eq!(*els[0].inner.backend_node_id.lock().await, Some(20));
        assert_eq!(
            els[1].inner.remote_object_id.lock().await.as_deref(),
            Some("R1")
        );
        assert_eq!(*els[1].inner.backend_node_id.lock().await, Some(21));
        conn.shutdown();
    }

    #[tokio::test]
    async fn many_or_empty_returns_empty_vec_on_timeout() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move {
                t.find_all()
                    .css(".missing")
                    .timeout(Duration::from_millis(120))
                    .many_or_empty()
                    .await
            }
        });

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => break,
                cmd = mock.expect_cmd("Runtime.evaluate") => {
                    mock.reply(
                        cmd,
                        json!({ "result": { "objectId": "RArrEmpty", "type": "object", "subtype": "array" } }),
                    )
                    .await;
                    let id_p = mock.expect_cmd("Runtime.getProperties").await;
                    mock.reply(id_p, json!({ "result": [
                        { "name": "length", "value": { "value": 0, "type": "number" } }
                    ] })).await;
                }
            }
        }

        let res = fut.await.unwrap().unwrap();
        assert!(
            res.is_empty(),
            "expected empty Vec on timeout, got len={}",
            res.len()
        );
        conn.shutdown();
    }

    #[tokio::test]
    async fn one_with_nth_picks_indexed_match() {
        // Two-match scenario: nth(1) must pick the second array entry,
        // not the first. Verifies the modifier wires through to the
        // resolve_many path.
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.find().css(".item").nth(1).one().await }
        });

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        mock.reply(
            id_p,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "R0", "type": "object", "subtype": "node" } },
                    { "name": "1", "value": { "objectId": "R1", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 2, "type": "number" } }
                ]
            }),
        )
        .await;

        // describeNode runs for each indexed entry as extract_array_refs
        // enumerates the array. Reply to both.
        let id_d0 = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R0");
        mock.reply(id_d0, json!({ "node": { "backendNodeId": 10 } }))
            .await;
        let id_d1 = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "R1");
        mock.reply(id_d1, json!({ "node": { "backendNodeId": 11 } }))
            .await;

        let el = fut.await.unwrap().unwrap();
        assert_eq!(
            el.inner.remote_object_id.lock().await.as_deref(),
            Some("R1")
        );
        assert_eq!(*el.inner.backend_node_id.lock().await, Some(11));
        conn.shutdown();
    }

    // --- A1: best_match (closest-text-length) -------------------------

    /// `.text(...).best_match().one()` must dispatch a text collector whose
    /// JS sorts candidates ascending by `abs(len(elementText) - len(needle))`
    /// so `.one()` takes the closest-length match. We assert the dispatched
    /// expression contains the distance sort (`Math.abs` + `.length` +
    /// `.sort`) and that `.one()` picks the first (post-sort) entry.
    #[tokio::test]
    async fn best_match_one_picks_closest_length() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.find().text("accept all").best_match().one().await }
        });

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains("Math.abs"),
            "best_match collector must score by abs distance; got: {sent}"
        );
        assert!(
            sent.contains(".length"),
            "best_match collector must compare text length; got: {sent}"
        );
        assert!(
            sent.contains(".sort"),
            "best_match collector must sort candidates; got: {sent}"
        );
        // Returns an array already sorted JS-side; .one() takes [0].
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        mock.reply(
            id_p,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "RBest", "type": "object", "subtype": "node" } },
                    { "name": "1", "value": { "objectId": "RFar", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 2, "type": "number" } }
                ]
            }),
        )
        .await;

        let id_d0 = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(mock.last_sent()["params"]["objectId"], "RBest");
        mock.reply(id_d0, json!({ "node": { "backendNodeId": 1 } }))
            .await;
        let id_d1 = mock.expect_cmd("DOM.describeNode").await;
        mock.reply(id_d1, json!({ "node": { "backendNodeId": 2 } }))
            .await;

        let el = fut.await.unwrap().unwrap();
        assert_eq!(
            el.inner.remote_object_id.lock().await.as_deref(),
            Some("RBest"),
            "best_match .one() must take the first (closest-length) candidate"
        );
        conn.shutdown();
    }

    /// `.css("x").best_match().one()` must behave exactly like the plain
    /// query: best_match is a no-op on non-text selectors (it only logs a
    /// debug warning). The dispatched JS must remain the bare
    /// `document.querySelectorAll` form with NO distance sort injected.
    #[tokio::test]
    async fn best_match_noop_on_css_logs() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.find().css("#b").best_match().one().await }
        });

        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        let sent = mock.last_sent()["params"]["expression"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            sent.contains("document.querySelectorAll") && sent.contains("#b"),
            "css path must dispatch plain querySelectorAll; got: {sent}"
        );
        assert!(
            !sent.contains("Math.abs"),
            "best_match must NOT inject a distance sort on css selectors; got: {sent}"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArr", "type": "object", "subtype": "array" } }),
        )
        .await;

        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        mock.reply(
            id_p,
            json!({
                "result": [
                    { "name": "0", "value": { "objectId": "R1", "type": "object", "subtype": "node" } },
                    { "name": "length", "value": { "value": 1, "type": "number" } }
                ]
            }),
        )
        .await;

        let id_d = mock.expect_cmd("DOM.describeNode").await;
        mock.reply(id_d, json!({ "node": { "backendNodeId": 7 } }))
            .await;

        let el = fut.await.unwrap().unwrap();
        assert_eq!(
            el.inner.remote_object_id.lock().await.as_deref(),
            Some("R1")
        );
        conn.shutdown();
    }

    // --- A1: include_frames (cross-frame fan-out) ---------------------

    /// `.include_frames().one()` (without best_match): resolve the main
    /// document scope first; on a miss, descend into the tab's registered
    /// frames in registry order and return the first frame hit. Here the
    /// main scope returns an empty array, then the single registered frame's
    /// session resolves one node — `.one()` must return it, dispatching the
    /// frame query on the frame's own session.
    // --- T4/T5: predicate builder accumulation (separate nested mod) ---
    #[cfg(test)]
    mod predicate_builder_tests {
        use super::super::*;
        use crate::query::predicate::{AttrPred, TextPred};

        fn bare() -> FindBuilder<'static> {
            FindBuilder {
                tab: None,
                element: None,
                frame: None,
                selector: None,
                predicates: Default::default(),
                timeout: DEFAULT_TIMEOUT,
                nth: None,
                visible_only: false,
                in_frame: None,
                include_frames: false,
                best_match: false,
            }
        }

        #[test]
        fn predicate_methods_accumulate() {
            let b = bare()
                .tag("div")
                .attr("data-x", "y")
                .attr_contains("class", "z")
                .has_attr("ready")
                .attr_regex("id", r"\d+")
                .containing_text("Buy")
                .text_equals("OK")
                .text_matches(r"^\$");
            assert_eq!(b.predicates.tag.as_deref(), Some("div"));
            assert_eq!(b.predicates.attrs.len(), 4);
            assert_eq!(b.predicates.texts.len(), 3);
            assert!(matches!(b.predicates.attrs[0], AttrPred::Exact(_, _)));
            assert!(matches!(b.predicates.texts[2], TextPred::Matches(_)));
        }

        fn bare_all() -> FindAllBuilder<'static> {
            FindAllBuilder {
                tab: None,
                element: None,
                frame: None,
                selector: None,
                predicates: Default::default(),
                timeout: DEFAULT_TIMEOUT,
                visible_only: false,
                in_frame: None,
                include_frames: false,
                best_match: false,
            }
        }

        #[test]
        fn find_all_predicates_accumulate() {
            let b = bare_all().tag("a").has_attr("href").containing_text("Next");
            assert_eq!(b.predicates.tag.as_deref(), Some("a"));
            assert_eq!(b.predicates.attrs.len(), 1);
            assert_eq!(b.predicates.texts.len(), 1);
        }

        // --- T6: conflict guard (unit-testable without a Tab) ----------

        /// A `PredicateSet` carrying a single `tag` — enough to make
        /// `is_empty()` false for the conflict-guard tests.
        fn tag_pred(tag: &str) -> PredicateSet {
            PredicateSet {
                tag: Some(tag.into()),
                ..Default::default()
            }
        }

        #[test]
        fn has_conflict_true_when_both_selector_and_predicate_set() {
            let sel = Some(SelectorKind::Css("div".into()));
            assert!(has_conflict(&sel, &tag_pred("span")));
        }

        #[test]
        fn has_conflict_false_for_selector_only() {
            let sel = Some(SelectorKind::Css("div".into()));
            assert!(!has_conflict(&sel, &PredicateSet::default()));
        }

        #[test]
        fn has_conflict_false_for_predicate_only() {
            assert!(!has_conflict(&None, &tag_pred("span")));
        }

        #[test]
        fn has_conflict_false_when_neither_set() {
            assert!(!has_conflict(&None, &PredicateSet::default()));
        }

        #[test]
        fn builder_with_css_and_tag_reports_conflict() {
            // The terminal reads `has_conflict(&self.selector,
            // &self.predicates)` first; mirror that here so the guard's
            // builder-level wiring is covered without a browser.
            let mut b = bare();
            b.selector = Some(SelectorKind::Css("div".into()));
            b = b.tag("span");
            assert!(has_conflict(&b.selector, &b.predicates));
        }

        #[test]
        fn describe_predicates_renders_css_and_filter() {
            // CSS-only (no post-filter) omits the filter clause.
            assert_eq!(
                describe_predicates(&tag_pred("button")),
                "predicate(css=button)"
            );
            // Adding a text predicate adds the JS post-filter to the label.
            let pred = PredicateSet {
                tag: Some("button".into()),
                texts: vec![TextPred::Contains("Buy".into())],
                ..Default::default()
            };
            let d = describe_predicates(&pred);
            assert!(d.starts_with("predicate(css=button, filter="), "{d}");
            assert!(d.contains("includes"), "{d}");
        }
    }

    #[tokio::test]
    async fn include_frames_one_falls_through_to_frame() {
        let (mut mock, conn) = MockConnection::pair();
        let tab_sess = SessionHandle::new(conn.clone(), "S_TAB");
        let frame_sess = SessionHandle::new(conn.clone(), "S_FRAME");
        let tab = Tab::new_for_test(tab_sess);

        // Register one frame on its own session so we can assert the
        // fall-through dispatch routes to S_FRAME.
        let frame = Frame::new(
            "F_CHILD".into(),
            Some("F_ROOT".into()),
            String::new(),
            None,
            frame_sess,
            std::sync::Arc::downgrade(&tab.inner),
        );
        tab.inner
            .frames
            .write()
            .await
            .insert("F_CHILD".into(), frame);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.find().css("button").include_frames().one().await }
        });

        // Main (Tab) scope resolves first and returns an empty array.
        let id_q = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(
            mock.last_sent()["sessionId"],
            "S_TAB",
            "main scope must resolve on the Tab's session first"
        );
        mock.reply(
            id_q,
            json!({ "result": { "objectId": "RArrEmpty", "type": "object", "subtype": "array" } }),
        )
        .await;
        let id_p = mock.expect_cmd("Runtime.getProperties").await;
        mock.reply(
            id_p,
            json!({ "result": [
                { "name": "length", "value": { "value": 0, "type": "number" } }
            ] }),
        )
        .await;

        // Miss on main → descend into the frame. Frame scope first allocates
        // its isolated world on the frame's session.
        let id_iso = mock.expect_cmd("Page.createIsolatedWorld").await;
        assert_eq!(
            mock.last_sent()["sessionId"],
            "S_FRAME",
            "frame fall-through must dispatch on the Frame's session"
        );
        assert_eq!(mock.last_sent()["params"]["frameId"], "F_CHILD");
        mock.reply(id_iso, json!({ "executionContextId": 4242 }))
            .await;

        let id_qf = mock.expect_cmd("Runtime.evaluate").await;
        assert_eq!(mock.last_sent()["sessionId"], "S_FRAME");
        assert_eq!(mock.last_sent()["params"]["contextId"], 4242);
        mock.reply(
            id_qf,
            json!({ "result": { "objectId": "RArrF", "type": "object", "subtype": "array" } }),
        )
        .await;
        let id_pf = mock.expect_cmd("Runtime.getProperties").await;
        mock.reply(
            id_pf,
            json!({ "result": [
                { "name": "0", "value": { "objectId": "RInFrame", "type": "object", "subtype": "node" } },
                { "name": "length", "value": { "value": 1, "type": "number" } }
            ] }),
        )
        .await;
        let id_df = mock.expect_cmd("DOM.describeNode").await;
        assert_eq!(
            mock.last_sent()["objectId"]
                .as_str()
                .or_else(|| mock.last_sent()["params"]["objectId"].as_str()),
            Some("RInFrame")
        );
        mock.reply(id_df, json!({ "node": { "backendNodeId": 88 } }))
            .await;

        let el = fut.await.unwrap().unwrap();
        assert_eq!(
            el.inner.remote_object_id.lock().await.as_deref(),
            Some("RInFrame"),
            "include_frames .one() must return the in-frame hit on main miss"
        );
        assert_eq!(*el.inner.backend_node_id.lock().await, Some(88));
        conn.shutdown();
    }
}
