//! Traits enabling generic code over Tab + Frame + Element.
//!
//! - [`Queryable`] — implemented by [`crate::Tab`], [`crate::Frame`], and
//!   [`crate::Element`]. Lets a generic function accept any query root.
//! - [`Evaluable`] — implemented by [`crate::Tab`] and [`crate::Frame`].
//!   Lets a generic function evaluate JS at either tab or frame
//!   granularity. [`crate::Element`] has its own `evaluate` shape (binds
//!   `el`) and is not part of this trait.
//!
//! ```no_run
//! use zendriver::Queryable;
//! async fn find_button<Q: Queryable + Sync>(q: &Q) -> zendriver::Result<zendriver::Element> {
//!     q.find().css("button").one().await
//! }
//! ```
//!
//! ## Inherent methods + trait impls — by design
//!
//! Each of [`crate::Tab`] / [`crate::Frame`] / [`crate::Element`] exposes
//! `find()` / `find_all()` / `evaluate()` / `evaluate_main()` as both
//! inherent methods and trait methods. The inherent methods carry the
//! authoritative docs + examples and don't require a `use` import; the
//! trait methods unlock generic helpers (`fn check<Q: Queryable>(q: &Q)`).
//! This duplication is intentional — removing the inherent shape would
//! force every call site to add `use zendriver::Queryable;` for what is
//! the high-frequency entry point of the library.

pub mod evaluable;
pub mod queryable;

pub use evaluable::Evaluable;
pub use queryable::Queryable;

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Compile-only: verify a generic function over Queryable accepts
    /// Tab, Frame, and Element.
    #[allow(dead_code)]
    async fn _accepts_queryable<Q: Queryable + Sync>(q: &Q) {
        let _ = q.find();
        let _ = q.find_all();
    }

    /// Compile-only: verify a generic function over Evaluable accepts
    /// Tab and Frame.
    #[allow(dead_code)]
    async fn _accepts_evaluable<E: Evaluable + Sync>(e: &E) {
        let _: crate::Result<i32> = e.evaluate("1+1").await;
        let _: crate::Result<i32> = e.evaluate_main("1+1").await;
    }

    /// Compile-only: ensure each concrete type satisfies the bounds.
    #[allow(dead_code)]
    fn _type_check_impls() {
        fn assert_queryable<T: Queryable>() {}
        fn assert_evaluable<T: Evaluable>() {}
        assert_queryable::<crate::Tab>();
        assert_queryable::<crate::Frame>();
        assert_queryable::<crate::Element>();
        assert_evaluable::<crate::Tab>();
        assert_evaluable::<crate::Frame>();
    }
}
