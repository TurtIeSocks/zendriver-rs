//! Network interception via the Fetch CDP domain.
//!
//! Two entry points:
//! - Rule-based: register declarative block/redirect/respond rules via
//!   `InterceptBuilder` and start a background actor.
//! - Stream: subscribe to paused requests and drive them manually.

pub mod actor;
pub mod builder;
pub mod error;
pub mod paused;
pub mod rule;
pub mod types;
pub mod url_pattern;

pub use error::InterceptionError;
pub use types::{
    AbortReason, RequestInfo, RequestOverrides, RequestStage, ResourceType, ResponseInfo,
};
