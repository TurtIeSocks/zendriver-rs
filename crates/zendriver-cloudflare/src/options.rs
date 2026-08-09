//! Caller-supplied inputs to the bypass: which markers identify the widget,
//! and how to click it.
//!
//! Cloudflare owns both. The class names, the hidden input's `name`, the
//! iframe host and the checkbox's position inside the widget are all theirs
//! to change, on their schedule, with no notice. A crate that bakes them in
//! is a crate that needs a release every time they move, and every user is
//! stuck on the old literals until it ships.
//!
//! So they are data here, not literals: [`TurnstileSelectors`] is what the
//! injected evaluators are built from, and [`ClickPolicy`] decides whether
//! and where to click. Both are plain structs with public fields — no
//! setters, no closures — so they survive a trip through JSON and can be
//! filled in by an agent, a config file, or a hotfix in the calling program.
//! For the part that cannot be expressed as data at all, there is
//! [`CloudflareBypass::on_click`](crate::CloudflareBypass::on_click).

use crate::detection::BoundingBox;

/// The page markers that identify a Cloudflare Turnstile widget.
///
/// Every injected evaluator is templated from these, so overriding one here
/// changes the detector, the poll loop and the scroll-and-measure step
/// together — there is no second copy to keep in sync.
///
/// ```no_run
/// # async fn ex(tab: &zendriver_transport::SessionHandle)
/// #   -> Result<(), zendriver_cloudflare::CloudflareError> {
/// use std::time::Duration;
/// use zendriver_cloudflare::{CloudflareBypass, TurnstileSelectors};
///
/// // Cloudflare moved the widget behind a new host: change the marker,
/// // not the crate.
/// let outcome = CloudflareBypass::new(tab)
///     .selectors(TurnstileSelectors {
///         iframe_src_contains: "challenges.example-cdn.com".into(),
///         ..TurnstileSelectors::default()
///     })
///     .wait_for_clearance(Duration::from_secs(30))
///     .await?;
/// # let _ = outcome;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnstileSelectors {
    /// Substring an `<iframe>`'s `src` must contain for that iframe to be
    /// the challenge widget. Matched with `String.prototype.includes`, so a
    /// bare hostname is enough. Defaults to `"challenges.cloudflare.com"`.
    /// An empty string disables the iframe signal — it never matches — since
    /// `includes("")` would otherwise match every iframe on the page and
    /// send the click at whichever one came first.
    pub iframe_src_contains: String,
    /// CSS selector for the challenge *container* — the element a site
    /// author drops into their own HTML for Turnstile to mount into. Used
    /// only as a presence signal, which is what separates "a challenge was
    /// on this page" from "this page has no gate at all". Defaults to
    /// `".cf-turnstile, .turnstile, [data-sitekey]"`. An empty string
    /// disables the container signal rather than raising in-page.
    pub container: String,
    /// CSS selectors for the hidden input that receives the clearance
    /// token, in **preference order**: the first one that matches an
    /// element wins, so a page carrying both the modern and the legacy
    /// input is read the modern way. Defaults to
    /// `["[name=\"cf-turnstile-response\"]", "[name=\"cf_challenge_response\"]"]`.
    /// Empty entries are skipped; an empty list disables token detection,
    /// leaving only the marker-vanished and deadline terminals.
    pub token_inputs: Vec<String>,
}

impl Default for TurnstileSelectors {
    fn default() -> Self {
        Self {
            iframe_src_contains: "challenges.cloudflare.com".to_string(),
            container: ".cf-turnstile, .turnstile, [data-sitekey]".to_string(),
            token_inputs: vec![
                "[name=\"cf-turnstile-response\"]".to_string(),
                "[name=\"cf_challenge_response\"]".to_string(),
            ],
        }
    }
}

/// Whether, how often and where to click the interactive widget.
///
/// ```no_run
/// # async fn ex(tab: &zendriver_transport::SessionHandle)
/// #   -> Result<(), zendriver_cloudflare::CloudflareError> {
/// use std::time::Duration;
/// use zendriver_cloudflare::{ClickPolicy, CloudflareBypass};
///
/// // Watch, do not touch: the page's own script drives the widget.
/// let outcome = CloudflareBypass::new(tab)
///     .click_policy(ClickPolicy {
///         max_attempts: 0,
///         ..ClickPolicy::default()
///     })
///     .wait_for_clearance(Duration::from_secs(30))
///     .await?;
/// # let _ = outcome;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ClickPolicy {
    /// How many clicks one `wait_for_clearance` run may spend on the
    /// widget. Cloudflare drops clicks that land while the widget is still
    /// booting, so a single attempt strands the run until the deadline.
    /// `0` disables clicking entirely — the run then watches only for a
    /// token or for the markers to vanish. Defaults to `3`.
    pub max_attempts: u32,
    /// Poll ticks to wait after a click before spending another attempt.
    /// Defaults to `4`, which is about two seconds at the default 500ms
    /// poll interval.
    pub retry_ticks: u32,
    /// Where to click horizontally inside the widget's box, as a fraction
    /// of its width. Defaults to `0.15` — 15% from the left edge, which is
    /// where the Turnstile checkbox sits.
    pub x_fraction: f64,
    /// Where to click vertically inside the widget's box, as a fraction of
    /// its height. Defaults to `0.50`.
    pub y_fraction: f64,
}

impl Default for ClickPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            retry_ticks: 4,
            x_fraction: 0.15,
            y_fraction: 0.50,
        }
    }
}

/// What a click is aimed at: the widget's post-scroll box, and the point
/// inside it that the [`ClickPolicy`] chose.
///
/// Handed to a caller-supplied
/// [`on_click`](crate::CloudflareBypass::on_click) handler, which may click
/// [`x`](Self::x) / [`y`](Self::y) however it likes, or ignore them and work
/// from [`bbox`](Self::bbox).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClickTarget {
    /// The widget's bounding box in viewport CSS pixels, measured *after*
    /// it was scrolled into view.
    pub bbox: BoundingBox,
    /// Viewport x of the chosen click point:
    /// `bbox.x + bbox.width * policy.x_fraction`.
    pub x: f64,
    /// Viewport y of the chosen click point:
    /// `bbox.y + bbox.height * policy.y_fraction`.
    pub y: f64,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Every default is stated in prose on its own field, repeated in the
    /// book's Cloudflare chapter, and read by callers who never set it. This
    /// is the one place that holds those numbers to what the docs claim —
    /// the driver's own tests deliberately assert against literals rather
    /// than against `Default`, so that changing a default here cannot
    /// quietly move an expectation there too.
    #[test]
    fn defaults_match_the_documented_prose() {
        let s = TurnstileSelectors::default();
        assert_eq!(s.iframe_src_contains, "challenges.cloudflare.com");
        assert_eq!(s.container, ".cf-turnstile, .turnstile, [data-sitekey]");
        assert_eq!(
            s.token_inputs,
            vec![
                "[name=\"cf-turnstile-response\"]".to_string(),
                "[name=\"cf_challenge_response\"]".to_string(),
            ],
            "the modern input is preferred over the legacy one, so order matters"
        );

        let p = ClickPolicy::default();
        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.retry_ticks, 4);
        assert_eq!(p.x_fraction, 0.15);
        assert_eq!(p.y_fraction, 0.50);
    }
}
