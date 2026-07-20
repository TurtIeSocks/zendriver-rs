//! Cubic Bezier path generation for realistic mouse movement.

#[derive(Debug, Clone)]
pub(crate) struct BezierPath {
    pub points: Vec<(f64, f64)>,
}

impl BezierPath {
    /// Build a cubic Bezier from `start` to `end`, with control points at
    /// ~25% and ~75% positions perturbed by up to `jitter_px`. The number of
    /// sample points scales with distance (one per ~5px, min 8, max 60).
    pub fn build(
        start: (f64, f64),
        end: (f64, f64),
        jitter_px: f64,
        rng: &mut impl rand::Rng,
    ) -> Self {
        let dx = end.0 - start.0;
        let dy = end.1 - start.1;
        let jitter = |rng: &mut dyn rand::RngCore| -> f64 {
            if jitter_px == 0.0 {
                return 0.0;
            }
            rand::Rng::gen_range(rng, -jitter_px..jitter_px)
        };
        let c1 = (
            start.0 + dx * 0.25 + jitter(rng),
            start.1 + dy * 0.25 + jitter(rng),
        );
        let c2 = (
            start.0 + dx * 0.75 + jitter(rng),
            start.1 + dy * 0.75 + jitter(rng),
        );
        let distance = (dx * dx + dy * dy).sqrt();
        let n_points = ((distance / 5.0).max(8.0) as usize).min(60);
        let mut points = Vec::with_capacity(n_points + 1);
        for i in 0..=n_points {
            let t = i as f64 / n_points as f64;
            points.push(cubic_bezier(start, c1, c2, end, t));
        }
        Self { points }
    }
}

fn cubic_bezier(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
    t: f64,
) -> (f64, f64) {
    let u = 1.0 - t;
    let (uu, tt) = (u * u, t * t);
    let (uuu, ttt) = (uu * u, tt * t);
    (
        uuu * p0.0 + 3.0 * uu * t * p1.0 + 3.0 * u * tt * p2.0 + ttt * p3.0,
        uuu * p0.1 + 3.0 * uu * t * p1.1 + 3.0 * u * tt * p2.1 + ttt * p3.1,
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn path_starts_at_start_and_ends_at_end() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let path = BezierPath::build((0.0, 0.0), (100.0, 100.0), 0.0, &mut rng);
        let first = path.points.first().expect("non-empty");
        let last = path.points.last().expect("non-empty");
        assert!((first.0 - 0.0).abs() < 1e-9);
        assert!((first.1 - 0.0).abs() < 1e-9);
        assert!((last.0 - 100.0).abs() < 1e-9);
        assert!((last.1 - 100.0).abs() < 1e-9);
    }

    #[test]
    fn path_point_count_scales_with_distance() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let short = BezierPath::build((0.0, 0.0), (10.0, 10.0), 0.0, &mut rng);
        let long = BezierPath::build((0.0, 0.0), (1000.0, 1000.0), 0.0, &mut rng);
        assert!(short.points.len() < long.points.len());
        assert!(short.points.len() >= 9); // min 8 + the start endpoint = 9
        assert!(long.points.len() <= 61); // max 60 + start endpoint = 61
    }

    #[test]
    fn zero_jitter_produces_smooth_path() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let path = BezierPath::build((0.0, 0.0), (100.0, 0.0), 0.0, &mut rng);
        // With zero jitter + a straight line, y should stay 0 (within float tolerance).
        for (_, y) in &path.points {
            assert!(y.abs() < 1e-9, "non-zero y on straight line: {y}");
        }
    }

    #[test]
    fn seeded_rng_produces_deterministic_path_snapshot() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(42);
        let path = BezierPath::build((0.0, 0.0), (100.0, 100.0), 2.0, &mut rng);
        insta::assert_yaml_snapshot!("bezier_path_seed_42", &path.points);
    }
}
