//! Capture `tracing` events emitted by the code under test — test-only.
//!
//! Some behaviour here is observable *only* as a log line. Where the fix for
//! a silent correction is "report the value the caller asked for, and say
//! why it looks wrong", a test that asserts the value alone stays green
//! against code that says nothing at all — so the saying has to be asserted
//! too.
//!
//! The crate's single copy: `patches`, `persona` and `profile` all capture
//! warnings, and the interest-cache rebuild below is the kind of correctness
//! detail that has to live in one place to stay right in all three.

use std::io::Write;
use std::sync::{Arc, Mutex};

/// A `MakeWriter` that appends every formatted event to a shared buffer.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Sink {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

/// Run `f` with a `WARN`-level capturing subscriber installed as the
/// thread-local default, returning everything it logged as one string.
pub(crate) fn captured_warnings(f: impl FnOnce()) -> String {
    let sink = Sink::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(sink.clone())
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        // `tracing` caches each callsite's interest globally the first time it
        // is evaluated. A site first reached with no subscriber installed —
        // which is every other test in this binary — caches "never" and would
        // stay invisible here, making this capture pass alone and fail in a
        // full run. Rebuilding re-evaluates every callsite against the
        // subscriber that is current now.
        tracing::callsite::rebuild_interest_cache();
        f();
    });
    let bytes = sink.0.lock().expect("log buffer poisoned").clone();
    String::from_utf8_lossy(&bytes).into_owned()
}
