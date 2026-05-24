//! Frame — handle to a single document frame within a [`crate::tab::Tab`].
//!
//! A [`Frame`] wraps the CDP `frameId` plus the [`zendriver_transport::SessionHandle`]
//! that should be used to dispatch commands against that frame. For the
//! main frame the session is the owning tab's session (same-process); for
//! out-of-process iframes (OOPIFs) it's a distinct child session attached
//! via `Target.attachedToTarget` (wired in T16).
//!
//! P4 Task 12 ships the bare struct + accessors + main-frame discovery via
//! [`crate::tab::Tab::main_frame`]. Evaluate / find / lifecycle / OOPIF /
//! navigation land in T13–T18.

use std::sync::{Arc, Weak};

use tokio::sync::{Mutex, RwLock};
use zendriver_transport::SessionHandle;

use crate::isolated_world::IsolatedWorldCache;
use crate::tab::TabInner;

pub mod lifecycle;
pub mod oopif;

/// Cheap-to-clone handle to a single document frame.
///
/// Construct via [`crate::tab::Tab::main_frame`] (top-level frame for a
/// tab); sub-frames and OOPIFs arrive via the lifecycle / OOPIF wiring
/// in later P4 tasks. All accessor methods operate on the inner `Arc`,
/// so cloning a `Frame` is a single refcount bump.
#[derive(Clone)]
pub struct Frame {
    inner: Arc<FrameInner>,
}

// Several fields below are populated at construction but consumed by later
// P4 tasks: `session` + `isolated_world` by Frame::evaluate (T13) and
// Frame::find (T14); `tab` by frame lifecycle / OOPIF wiring (T15+T16).
// Silencing dead-code until those land keeps clippy clean without dropping
// the (already-correct) plumbing.
#[allow(dead_code)]
pub(crate) struct FrameInner {
    /// CDP `frameId` (e.g. `"F0"`, a hex string at runtime). Stable for the
    /// lifetime of the frame.
    pub(crate) frame_id: String,
    /// Parent frame's CDP `frameId`. `None` for the main (top-level) frame;
    /// `Some` for every sub-frame and OOPIF.
    pub(crate) parent_frame_id: Option<String>,
    /// Last-known document URL for this frame. Behind an [`RwLock`] because
    /// the lifecycle subscriber task (T15) mutates it on
    /// `Page.frameNavigated` while readers concurrently call [`Frame::url`].
    pub(crate) url: RwLock<String>,
    /// `<frame name>` / `<iframe name>` attribute if present. Captured at
    /// construction time; the spec does not currently track renames after
    /// the fact since `Page.frameNavigated` does not carry the name field.
    pub(crate) name: Option<String>,
    /// CDP session used to dispatch commands against this frame. The main
    /// frame shares the owning tab's session; OOPIFs (T16) attach to a
    /// distinct child session whose handle is plumbed in at construction.
    pub(crate) session: SessionHandle,
    /// Per-frame isolated-world cache. Same shape as the tab-level cache —
    /// `Frame::evaluate` (T13) populates `executionContextId` on first call
    /// via `Page.createIsolatedWorld { frameId: self.frame_id }` and reuses
    /// it on subsequent calls. Distinct from the tab-level cache so that
    /// per-frame contexts don't collide when the tab has multiple frames.
    pub(crate) isolated_world: Mutex<IsolatedWorldCache>,
    /// Weak ref to the owning tab. Used by later P4 tasks (lifecycle
    /// updates, frame-tree walks). `Weak` so that a long-held `Frame`
    /// clone does not pin the tab alive past its public lifetime.
    pub(crate) tab: Weak<TabInner>,
}

impl Frame {
    /// Construct a `Frame` from its CDP identity + the session that should
    /// dispatch commands against it.
    ///
    /// Called by [`crate::tab::Tab::main_frame`] (main-frame path, shares
    /// the tab's session) and — in later P4 tasks — by the lifecycle
    /// subscriber (sub-frame attach) and the OOPIF attach observer
    /// (distinct child session).
    pub(crate) fn new(
        frame_id: String,
        parent_frame_id: Option<String>,
        url: String,
        name: Option<String>,
        session: SessionHandle,
        tab: Weak<TabInner>,
    ) -> Self {
        Self {
            inner: Arc::new(FrameInner {
                frame_id,
                parent_frame_id,
                url: RwLock::new(url),
                name,
                session,
                isolated_world: Mutex::new(IsolatedWorldCache::default()),
                tab,
            }),
        }
    }

    /// The frame's CDP `frameId`. Stable for the lifetime of the frame.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.inner.frame_id
    }

    /// The frame's parent CDP `frameId`. `None` iff this is the main
    /// (top-level) frame for the owning tab.
    #[must_use]
    pub fn parent_id(&self) -> Option<&str> {
        self.inner.parent_frame_id.as_deref()
    }

    /// The frame's `name` attribute (`<iframe name="...">`). `None` for
    /// frames without an explicit name (including the main frame in most
    /// cases).
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.inner.name.as_deref()
    }

    /// `true` iff this is the main (top-level) frame for its owning tab —
    /// equivalent to `parent_id().is_none()`.
    #[must_use]
    pub fn is_main(&self) -> bool {
        self.inner.parent_frame_id.is_none()
    }

    /// The frame's current document URL. Snapshot under an `RwLock`; cheap
    /// to clone the resulting `String`. The lifecycle subscriber (T15)
    /// keeps this fresh on `Page.frameNavigated` events; until T15 lands
    /// the value reflects the construction-time URL only.
    pub async fn url(&self) -> String {
        self.inner.url.read().await.clone()
    }
}
