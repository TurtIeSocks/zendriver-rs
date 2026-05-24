//! Public traits enabling generic code over Tab + Frame + Element.

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
