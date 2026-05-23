//! zendriver — async, undetectable Chrome automation over CDP.
//!
//! Phase 1 surface: see the [module-level docs] on each public type.
//!
//! [module-level docs]: crate

#![cfg_attr(docsrs, feature(doc_cfg))]

// Module skeleton; populated in subsequent tasks.
pub mod browser;
pub mod element;
pub mod error;
pub mod query;
pub mod tab;
