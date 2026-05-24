//! Shared element-query surface across types that own a query scope.

use crate::query::{FindAllBuilder, FindBuilder};

/// Types that expose `find()` + `find_all()` queries scoped to themselves.
///
/// Implemented by [`crate::Tab`] (queries scoped to main frame),
/// [`crate::Frame`] (queries scoped to that frame's contextId), and
/// [`crate::Element`] (queries scoped to the element's subtree). Use this
/// trait when writing helpers that should work against any of the three.
///
/// # Examples
///
/// ```no_run
/// use zendriver::Queryable;
/// async fn first_link<Q: Queryable + Sync>(q: &Q) -> zendriver::Result<zendriver::Element> {
///     q.find().css("a").one().await
/// }
/// ```
pub trait Queryable {
    /// Start a single-element query. See [`crate::Tab::find`] for the
    /// terminal + modifier surface.
    fn find(&self) -> FindBuilder<'_>;

    /// Start a multi-element query. See [`crate::Tab::find_all`] for the
    /// terminal + modifier surface.
    fn find_all(&self) -> FindAllBuilder<'_>;
}
