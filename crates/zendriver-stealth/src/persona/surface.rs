//! Surface + Strategy + per-surface kind resolution.

use serde::{Deserialize, Serialize};

/// A fingerprint surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Surface {
    Canvas,
    Webgl,
    Audio,
    Fonts,
    ClientRects,
    Webrtc,
    Hardware,
}

/// How a resolved persona is applied to a surface in the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Strategy {
    Native,
    Seeded,
    Random,
    Block,
    Value,
}

/// The semantic family a surface belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Noise,
    Value,
    Policy,
}

impl Surface {
    pub fn kind(self) -> SurfaceKind {
        match self {
            Surface::Canvas | Surface::Audio | Surface::ClientRects => SurfaceKind::Noise,
            Surface::Webgl | Surface::Fonts | Surface::Hardware => SurfaceKind::Value,
            Surface::Webrtc => SurfaceKind::Policy,
        }
    }

    /// Default strategy when none is set.
    pub fn default_strategy(self) -> Strategy {
        match self.kind() {
            SurfaceKind::Noise => Strategy::Seeded,
            SurfaceKind::Value => Strategy::Value,
            SurfaceKind::Policy => Strategy::Block,
        }
    }

    /// Resolve a requested strategy against this surface's kind.
    /// Meaningless combos warn and fall back to the kind default
    /// (least-opinionated: never error).
    pub fn resolve_strategy(self, requested: Option<Strategy>) -> Strategy {
        let req = match requested {
            None => return self.default_strategy(),
            Some(s) => s,
        };
        let ok = matches!(
            (self.kind(), req),
            (_, Strategy::Native)
                | (_, Strategy::Block)
                | (SurfaceKind::Noise, Strategy::Seeded | Strategy::Random)
                | (SurfaceKind::Value, Strategy::Value)
                | (SurfaceKind::Policy, Strategy::Value)
        );
        if ok {
            req
        } else {
            tracing::warn!(
                surface = ?self,
                requested = ?req,
                "strategy not meaningful for surface kind; using default"
            );
            self.default_strategy()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_default_is_seeded() {
        assert_eq!(Surface::Canvas.resolve_strategy(None), Strategy::Seeded);
    }

    #[test]
    fn value_strategy_on_noise_warns_and_falls_back() {
        // Value is meaningless for a noise surface → falls back to Seeded.
        assert_eq!(
            Surface::Canvas.resolve_strategy(Some(Strategy::Value)),
            Strategy::Seeded
        );
    }

    #[test]
    fn native_and_block_always_pass() {
        assert_eq!(
            Surface::Webgl.resolve_strategy(Some(Strategy::Native)),
            Strategy::Native
        );
        assert_eq!(
            Surface::Audio.resolve_strategy(Some(Strategy::Block)),
            Strategy::Block
        );
    }

    #[test]
    fn webrtc_default_is_block() {
        assert_eq!(Surface::Webrtc.resolve_strategy(None), Strategy::Block);
    }
}
