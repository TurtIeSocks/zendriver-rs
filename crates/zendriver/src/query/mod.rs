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

pub mod actionability;
pub mod modifiers;
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

use tokio::time::Instant;

use crate::element::Element;
use crate::error::{Result, ZendriverError};
use crate::frame::Frame;
use crate::query::selectors::{QueryScope, SelectorKind};
use crate::tab::Tab;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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
    pub(crate) timeout: Duration,
    pub(crate) nth: Option<usize>,
    pub(crate) visible_only: bool,
    /// Frame override populated by [`FindBuilder::in_frame`]. When set,
    /// the terminal swaps the scope to [`QueryScope::Frame`] before
    /// dispatch, routing commands through the frame's own session —
    /// even if the builder was originally rooted on a Tab. Element
    /// scope still takes precedence over both.
    pub(crate) in_frame: Option<&'scope Frame>,
}

impl<'scope> FindBuilder<'scope> {
    pub(crate) fn new_for_tab(tab: &'scope Tab) -> Self {
        Self {
            tab: Some(tab),
            element: None,
            frame: None,
            selector: None,
            timeout: DEFAULT_TIMEOUT,
            nth: None,
            visible_only: false,
            in_frame: None,
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
            timeout: DEFAULT_TIMEOUT,
            nth: None,
            visible_only: false,
            in_frame: None,
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
            timeout: DEFAULT_TIMEOUT,
            nth: None,
            visible_only: false,
            in_frame: None,
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
            timeout: self.timeout,
            nth: self.nth,
            visible_only: self.visible_only,
            in_frame: Some(frame),
        }
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
        let selector = self.selector.ok_or_else(|| {
            ZendriverError::Navigation(
                "FindBuilder requires a selector (.css/.xpath/.text/.role/...)".into(),
            )
        })?;
        let deadline = Instant::now() + self.timeout;
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
                ))
            }
        };
        let want_nth = self.nth.unwrap_or(0);
        loop {
            let candidates = selector.resolve_many(&scope).await?;

            // Visible-only filter: TODO(T16) — depends on
            // `actionability::check_visible`, which depends on
            // `Element::call_on_main`. Until that lands, treat every
            // candidate as visible so the wider FindBuilder API can ship.
            let _ = self.visible_only;
            let filtered = candidates;

            if let Some(picked) = filtered.into_iter().nth(want_nth) {
                return Ok(Element::synthesize_query(
                    picked, &scope, &selector, want_nth,
                ));
            }
            if Instant::now() >= deadline {
                return Err(ZendriverError::ElementNotFound {
                    selector: describe_selector(&selector),
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
    pub(crate) timeout: Duration,
    pub(crate) visible_only: bool,
    /// Frame override populated by [`FindAllBuilder::in_frame`]. See
    /// the corresponding field on [`FindBuilder`] for the precedence
    /// rationale.
    pub(crate) in_frame: Option<&'scope Frame>,
}

impl<'scope> FindAllBuilder<'scope> {
    pub(crate) fn new_for_tab(tab: &'scope Tab) -> Self {
        Self {
            tab: Some(tab),
            element: None,
            frame: None,
            selector: None,
            timeout: DEFAULT_TIMEOUT,
            visible_only: false,
            in_frame: None,
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
            timeout: DEFAULT_TIMEOUT,
            visible_only: false,
            in_frame: None,
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
            timeout: DEFAULT_TIMEOUT,
            visible_only: false,
            in_frame: None,
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
            timeout: self.timeout,
            visible_only: self.visible_only,
            in_frame: Some(frame),
        }
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
        let selector = self.selector.ok_or_else(|| {
            ZendriverError::Navigation(
                "FindAllBuilder requires a selector (.css/.xpath/.text/.role/...)".into(),
            )
        })?;
        let deadline = Instant::now() + self.timeout;
        // See `FindBuilder::one` for the precedence rationale.
        let scope = match (self.element, self.in_frame, self.frame, self.tab) {
            (Some(el), _, _, _) => QueryScope::Element(el),
            (None, Some(fr), _, _) => QueryScope::Frame(fr),
            (None, None, Some(fr), _) => QueryScope::Frame(fr),
            (None, None, None, Some(tab)) => QueryScope::Tab(tab),
            (None, None, None, None) => {
                return Err(ZendriverError::Navigation(
                    "FindAllBuilder has no scope (no tab, element, or frame)".into(),
                ))
            }
        };
        loop {
            let candidates = selector.resolve_many(&scope).await?;

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
                    .map(|(i, r)| Element::synthesize_query(r, &scope, &selector, i))
                    .collect();
                return Ok(elements);
            }
            if Instant::now() >= deadline {
                return Err(ZendriverError::ElementNotFound {
                    selector: describe_selector(&selector),
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

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::testing::MockConnection;
    use zendriver_transport::SessionHandle;

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
            mock.last_sent()["sessionId"], "S_FRAME",
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
}
