//! Shared element-query surface across types that own a query scope.

use crate::query::{FindAllBuilder, FindBuilder};

/// Types that expose `find()` + `find_all()` queries scoped to themselves.
///
/// Implemented by [`crate::Tab`] (queries scoped to main frame),
/// [`crate::Frame`] (queries scoped to that frame's contextId), and
/// [`crate::Element`] (queries scoped to the element's subtree).
pub trait Queryable {
    /// Start a single-element query.
    fn find(&self) -> FindBuilder<'_>;

    /// Start a multi-element query.
    fn find_all(&self) -> FindAllBuilder<'_>;
}
