//! Chainable PDF export + MHTML snapshot for a [`Tab`].
//!
//! [`PdfBuilder`] mirrors [`crate::ScreenshotBuilder`]: a builder bound to a
//! `&Tab` with fluent setters for paper geometry, margins, scale, background
//! rendering, and page ranges. Terminate with [`PdfBuilder::bytes`] (raw PDF
//! bytes) or [`PdfBuilder::save`] (write to file).
//!
//! Construct via [`Tab::pdf_builder`]; for a parameterless A4-portrait export,
//! [`Tab::print_to_pdf`] is the shortcut. For a single-file MHTML archive of
//! the rendered page, see [`Tab::snapshot_mhtml`] / [`Tab::save_snapshot`].
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! tab.goto("https://example.com").await?;
//! tab.pdf_builder()
//!     .landscape(true)
//!     .print_background(true)
//!     .save("page.pdf").await?;
//! # Ok(()) }
//! ```
//!
//! ## Defaults
//!
//! A fresh [`PdfBuilder`] sends no geometry overrides — Chrome falls back to
//! its own `Page.printToPDF` defaults (US-Letter-ish portrait, default
//! margins, `printBackground: false`, `scale: 1`). Set only the knobs you
//! care about; unset fields are omitted from the wire payload so Chrome's
//! defaults apply. Paper dimensions and margins are in **inches**, matching
//! the CDP contract.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Map, Value, json};

use crate::error::{Result, ZendriverError};
use crate::tab::Tab;

/// Chainable PDF export bound to a [`Tab`].
///
/// Default state: every knob unset, so [`Self::bytes`] dispatches
/// `Page.printToPDF` with an empty parameter object and Chrome applies its
/// own defaults. Terminate the chain with [`Self::bytes`] (raw PDF bytes) or
/// [`Self::save`] (write to file).
#[derive(Debug)]
pub struct PdfBuilder<'tab> {
    tab: &'tab Tab,
    landscape: Option<bool>,
    print_background: Option<bool>,
    scale: Option<f64>,
    paper_width: Option<f64>,
    paper_height: Option<f64>,
    margin_top: Option<f64>,
    margin_bottom: Option<f64>,
    margin_left: Option<f64>,
    margin_right: Option<f64>,
    page_ranges: Option<String>,
    prefer_css_page_size: Option<bool>,
}

impl<'tab> PdfBuilder<'tab> {
    /// Construct a fresh builder bound to `tab` with no overrides.
    ///
    /// Every knob starts unset; [`Self::bytes`] then dispatches
    /// `Page.printToPDF` with an empty parameter object so Chrome uses its
    /// own defaults. Most users go through [`Tab::pdf_builder`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use zendriver::PdfBuilder;
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let bytes = PdfBuilder::new(&tab).bytes().await?;
    /// # let _ = bytes;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn new(tab: &'tab Tab) -> Self {
        Self {
            tab,
            landscape: None,
            print_background: None,
            scale: None,
            paper_width: None,
            paper_height: None,
            margin_top: None,
            margin_bottom: None,
            margin_left: None,
            margin_right: None,
            page_ranges: None,
            prefer_css_page_size: None,
        }
    }

    /// Orient the page in landscape (`true`) or portrait (`false`).
    ///
    /// Maps to `Page.printToPDF { landscape }`. Unset leaves Chrome's
    /// default (portrait).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().landscape(true).save("wide.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn landscape(mut self, on: bool) -> Self {
        self.landscape = Some(on);
        self
    }

    /// Render the page's background graphics (colors / images).
    ///
    /// Maps to `Page.printToPDF { printBackground }`. Off by default in
    /// Chrome, so set this to `true` for WYSIWYG output.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().print_background(true).save("solid.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn print_background(mut self, on: bool) -> Self {
        self.print_background = Some(on);
        self
    }

    /// Scale the rendered content (1.0 = 100%).
    ///
    /// Maps to `Page.printToPDF { scale }`. Chrome clamps to roughly
    /// `0.1..=2.0`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().scale(0.75).save("smaller.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn scale(mut self, scale: f64) -> Self {
        self.scale = Some(scale);
        self
    }

    /// Paper width in **inches**.
    ///
    /// Maps to `Page.printToPDF { paperWidth }`. Pair with
    /// [`Self::paper_height`] for a custom sheet; e.g. A4 portrait is
    /// `8.27 × 11.69`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().paper_width(8.27).paper_height(11.69).save("a4.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn paper_width(mut self, inches: f64) -> Self {
        self.paper_width = Some(inches);
        self
    }

    /// Paper height in **inches**.
    ///
    /// Maps to `Page.printToPDF { paperHeight }`. See [`Self::paper_width`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().paper_width(8.27).paper_height(11.69).save("a4.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn paper_height(mut self, inches: f64) -> Self {
        self.paper_height = Some(inches);
        self
    }

    /// Top margin in **inches**.
    ///
    /// Maps to `Page.printToPDF { marginTop }`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().margin_top(0.5).save("margins.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn margin_top(mut self, inches: f64) -> Self {
        self.margin_top = Some(inches);
        self
    }

    /// Bottom margin in **inches**.
    ///
    /// Maps to `Page.printToPDF { marginBottom }`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().margin_bottom(0.5).save("margins.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn margin_bottom(mut self, inches: f64) -> Self {
        self.margin_bottom = Some(inches);
        self
    }

    /// Left margin in **inches**.
    ///
    /// Maps to `Page.printToPDF { marginLeft }`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().margin_left(0.5).save("margins.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn margin_left(mut self, inches: f64) -> Self {
        self.margin_left = Some(inches);
        self
    }

    /// Right margin in **inches**.
    ///
    /// Maps to `Page.printToPDF { marginRight }`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().margin_right(0.5).save("margins.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn margin_right(mut self, inches: f64) -> Self {
        self.margin_right = Some(inches);
        self
    }

    /// Restrict output to the given page ranges (e.g. `"1-5, 8, 11-13"`).
    ///
    /// Maps to `Page.printToPDF { pageRanges }`. Empty / unset prints all
    /// pages.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().page_ranges("1-3").save("first3.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn page_ranges(mut self, ranges: impl Into<String>) -> Self {
        self.page_ranges = Some(ranges.into());
        self
    }

    /// Prefer any CSS-declared `@page` size over [`Self::paper_width`] /
    /// [`Self::paper_height`].
    ///
    /// Maps to `Page.printToPDF { preferCSSPageSize }`. When `true`, an
    /// explicit `paper_width` / `paper_height` is ignored in favor of the
    /// page's own `@page { size: ... }` rule.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().prefer_css_page_size(true).save("css.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn prefer_css_page_size(mut self, on: bool) -> Self {
        self.prefer_css_page_size = Some(on);
        self
    }

    /// Execute the export and return the raw PDF bytes.
    ///
    /// Sends `Page.printToPDF` with every set knob and base64-decodes the
    /// response's `data` field into the returned `Vec<u8>`. Unset knobs are
    /// omitted so Chrome's defaults apply.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when Chrome returns no PDF data
    /// or malformed base64.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// let bytes = tab.pdf_builder().bytes().await?;
    /// tokio::fs::write("out.pdf", bytes).await?;
    /// # Ok(()) }
    /// ```
    pub async fn bytes(self) -> Result<Vec<u8>> {
        let mut params = Map::new();
        if let Some(v) = self.landscape {
            params.insert("landscape".to_string(), Value::Bool(v));
        }
        if let Some(v) = self.print_background {
            params.insert("printBackground".to_string(), Value::Bool(v));
        }
        if let Some(v) = self.scale {
            params.insert("scale".to_string(), json!(v));
        }
        if let Some(v) = self.paper_width {
            params.insert("paperWidth".to_string(), json!(v));
        }
        if let Some(v) = self.paper_height {
            params.insert("paperHeight".to_string(), json!(v));
        }
        if let Some(v) = self.margin_top {
            params.insert("marginTop".to_string(), json!(v));
        }
        if let Some(v) = self.margin_bottom {
            params.insert("marginBottom".to_string(), json!(v));
        }
        if let Some(v) = self.margin_left {
            params.insert("marginLeft".to_string(), json!(v));
        }
        if let Some(v) = self.margin_right {
            params.insert("marginRight".to_string(), json!(v));
        }
        if let Some(v) = self.page_ranges {
            params.insert("pageRanges".to_string(), Value::String(v));
        }
        if let Some(v) = self.prefer_css_page_size {
            params.insert("preferCSSPageSize".to_string(), Value::Bool(v));
        }

        let res = self
            .tab
            .call("Page.printToPDF", Value::Object(params))
            .await?;
        let data = res
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZendriverError::Navigation("Page.printToPDF returned no data".into()))?;
        BASE64
            .decode(data)
            .map_err(|e| ZendriverError::Navigation(format!("invalid base64 in pdf: {e}")))
    }

    /// Execute the export and write the raw PDF bytes to `path`.
    ///
    /// Convenience wrapper over [`Self::bytes`] + [`tokio::fs::write`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.pdf_builder().landscape(true).save("page.pdf").await?;
    /// # Ok(()) }
    /// ```
    pub async fn save(self, path: impl AsRef<Path>) -> Result<()> {
        let bytes = self.bytes().await?;
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }
}

impl Tab {
    /// Start a chainable PDF export of this tab.
    ///
    /// Chain paper / margin / scale / orientation options, then call
    /// [`PdfBuilder::bytes`] (raw PDF bytes) or [`PdfBuilder::save`] (write to
    /// file) to execute the export. For a parameterless A4-portrait save, see
    /// [`Tab::print_to_pdf`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.pdf_builder()
    ///     .landscape(true)
    ///     .print_background(true)
    ///     .save("page.pdf").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn pdf_builder(&self) -> PdfBuilder<'_> {
        PdfBuilder::new(self)
    }

    /// Export this tab as an A4-portrait PDF and write it to `path`.
    ///
    /// Convenience wrapper that drives [`Tab::pdf_builder`] with A4 paper
    /// dimensions (`8.27 × 11.69` inches) and saves. For landscape, custom
    /// paper, margins, scale, or page ranges, drive [`Tab::pdf_builder`]
    /// directly.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.print_to_pdf("page.pdf").await?;
    /// # Ok(()) }
    /// ```
    pub async fn print_to_pdf(&self, path: impl AsRef<Path>) -> Result<()> {
        self.pdf_builder()
            .paper_width(8.27)
            .paper_height(11.69)
            .save(path)
            .await
    }

    /// Capture a single-file MHTML archive of the rendered page.
    ///
    /// Sends `Page.captureSnapshot { format: "mhtml" }` and returns the
    /// archive's `data` string verbatim (the full MHTML document, inlining
    /// resources). Write it with [`Tab::save_snapshot`] or persist it
    /// yourself.
    ///
    /// # Errors
    ///
    /// Returns [`ZendriverError::Navigation`] when Chrome returns no snapshot
    /// data.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// let mhtml = tab.snapshot_mhtml().await?;
    /// assert!(mhtml.contains("MIME"));
    /// # Ok(()) }
    /// ```
    pub async fn snapshot_mhtml(&self) -> Result<String> {
        let res = self
            .call("Page.captureSnapshot", json!({ "format": "mhtml" }))
            .await?;
        let data = res.get("data").and_then(|v| v.as_str()).ok_or_else(|| {
            ZendriverError::Navigation("Page.captureSnapshot returned no data".into())
        })?;
        Ok(data.to_string())
    }

    /// Capture an MHTML snapshot and write it to `path`.
    ///
    /// Convenience wrapper over [`Tab::snapshot_mhtml`] + [`tokio::fs::write`].
    /// The MHTML is a self-contained archive of the page (inlined resources),
    /// openable directly in Chrome.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn ex() -> zendriver::Result<()> {
    /// # let browser = zendriver::Browser::builder().launch().await?;
    /// # let tab = browser.main_tab();
    /// tab.goto("https://example.com").await?;
    /// tab.save_snapshot("page.mhtml").await?;
    /// # Ok(()) }
    /// ```
    pub async fn save_snapshot(&self, path: impl AsRef<Path>) -> Result<()> {
        let mhtml = self.snapshot_mhtml().await?;
        tokio::fs::write(path, mhtml).await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use zendriver_transport::SessionHandle;
    use zendriver_transport::testing::MockConnection;

    /// `landscape(true)` surfaces `landscape: true` on the `Page.printToPDF`
    /// payload; the base64 `data` reply is decoded to raw bytes.
    #[tokio::test]
    async fn pdf_builder_landscape_dispatches_print_to_pdf() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { PdfBuilder::new(&t).landscape(true).bytes().await }
        });

        let id = mock.expect_cmd("Page.printToPDF").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["landscape"], true);
        // "%PDF" → b"%PDF" once base64-decoded.
        mock.reply(id, json!({ "data": "JVBERg==" })).await;

        let bytes = fut.await.unwrap().unwrap();
        assert_eq!(bytes, b"%PDF");
        conn.shutdown();
    }

    /// `print_to_pdf(path)` exports A4 portrait and writes the decoded bytes
    /// to the given file.
    #[tokio::test]
    async fn print_to_pdf_saves_file() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let dir = std::env::temp_dir();
        let path = dir.join(format!("zendriver_pdf_test_{}.pdf", std::process::id()));
        let path_for_task = path.clone();

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.print_to_pdf(&path_for_task).await }
        });

        let id = mock.expect_cmd("Page.printToPDF").await;
        let sent = mock.last_sent();
        // A4 portrait dimensions in inches.
        assert_eq!(sent["params"]["paperWidth"], 8.27);
        assert_eq!(sent["params"]["paperHeight"], 11.69);
        mock.reply(id, json!({ "data": "JVBERg==" })).await;

        fut.await.unwrap().unwrap();
        let written = std::fs::read(&path).unwrap();
        assert_eq!(written, b"%PDF");
        let _ = std::fs::remove_file(&path);
        conn.shutdown();
    }

    /// `save_snapshot(path)` dispatches `Page.captureSnapshot { format:
    /// "mhtml" }` and writes the returned MHTML string to the file.
    #[tokio::test]
    async fn save_snapshot_dispatches_capture_snapshot_mhtml() {
        let (mut mock, conn) = MockConnection::pair();
        let sess = SessionHandle::new(conn.clone(), "S1");
        let tab = Tab::new_for_test(sess);

        let dir = std::env::temp_dir();
        let path = dir.join(format!("zendriver_mhtml_test_{}.mhtml", std::process::id()));
        let path_for_task = path.clone();

        let fut = tokio::spawn({
            let t = tab.clone();
            async move { t.save_snapshot(&path_for_task).await }
        });

        let id = mock.expect_cmd("Page.captureSnapshot").await;
        let sent = mock.last_sent();
        assert_eq!(sent["params"]["format"], "mhtml");
        mock.reply(
            id,
            json!({ "data": "From: <Saved by Chrome>\r\nMIME-Version: 1.0\r\n" }),
        )
        .await;

        fut.await.unwrap().unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("MIME-Version"));
        let _ = std::fs::remove_file(&path);
        conn.shutdown();
    }
}
