//! Event expectation helpers (`expect_request` / `expect_response` /
//! `expect_dialog` / `expect_download`).
//!
//! Each helper registers a one-shot subscription on a Tab's CDP event stream
//! and resolves with the first matching event. [`UrlMatcher`] is the shared
//! pattern type used by request/response expectations.
//!
//! ```no_run
//! # async fn ex() -> zendriver::Result<()> {
//! # let browser = zendriver::Browser::builder().launch().await?;
//! # let tab = browser.main_tab();
//! // Pre-register before triggering the action that causes the request.
//! let exp = tab.expect_response("/api/users");
//! tab.find().css("button#load").one().await?.click().await?;
//! let resp = exp.await?;
//! let body = resp.body().await?;
//! # let _ = body;
//! # Ok(()) }
//! ```

pub mod dialog;
pub mod download;
pub mod request;
pub mod response;

pub use crate::url_matcher::UrlMatcher;
