//! Network interception via the `Fetch` CDP domain.
//!
//! See the [Interception chapter](https://turtiesocks.github.io/zendriver-rs/interception.html)
//! of the [zendriver-rs user guide](https://turtiesocks.github.io/zendriver-rs/)
//! for narrative examples, glob/regex pattern semantics, and streaming-mode
//! recipes.
//!
//! Two entry points, both built off [`InterceptBuilder`]:
//!
//! - **Rule-based** — chain
//!   [`block`](builder::InterceptBuilder::block) /
//!   [`redirect`](builder::InterceptBuilder::redirect) /
//!   [`respond`](builder::InterceptBuilder::respond) /
//!   [`modify_request`](builder::InterceptBuilder::modify_request) and call
//!   [`start`](builder::InterceptBuilder::start). The returned
//!   [`InterceptHandle`] tears the actor down on drop.
//! - **Stream** — call
//!   [`subscribe`](builder::InterceptBuilder::subscribe) to get a `Stream` of
//!   [`PausedRequest`]; release each pause by calling one of
//!   [`continue_`](PausedRequest::continue_), [`abort`](PausedRequest::abort),
//!   [`respond`](PausedRequest::respond), or
//!   [`modify_and_continue`](PausedRequest::modify_and_continue).
//!
//! ```no_run
//! # async fn ex(tab: &zendriver_transport::SessionHandle)
//! #   -> Result<(), zendriver_interception::InterceptionError> {
//! use zendriver_interception::InterceptBuilder;
//!
//! let _handle = InterceptBuilder::new(tab)
//!     .block("*/ads/*")?
//!     .redirect("*/old/*", "https://example.com/new/")?
//!     .start();
//! // _handle stays in scope -> interception live.
//! # Ok(()) }
//! ```

pub mod actor;
pub mod builder;
pub mod error;
pub mod paused;
pub mod rule;
pub mod types;
pub mod url_pattern;

pub use actor::InterceptHandle;
pub use builder::{InterceptBuilder, RequestPattern};
pub use error::InterceptionError;
pub use paused::PausedRequest;
pub use rule::Rule;
pub use types::{
    AbortReason, RequestInfo, RequestOverrides, RequestStage, ResourceType, ResponseInfo,
};
