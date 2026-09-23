use std::time::Duration;

/// A bounded exponential backoff curve.
///
/// `delay(0)` is [`Backoff::base`], and every attempt after that multiplies by
/// [`Backoff::factor`] until [`Backoff::maximum`] caps it. Jitter only ever
/// subtracts, so the maximum stays a true upper bound.
///
/// ```
/// use abnegate_http::Backoff;
/// use std::time::Duration;
///
/// let backoff = Backoff {
///     base: Duration::from_secs(1),
///     factor: 2,
///     maximum: Duration::from_secs(30),
///     jitter: 0.0,
/// };
///
/// assert_eq!(backoff.delay_with(0, 1.0), Duration::from_secs(1));
/// assert_eq!(backoff.delay_with(3, 1.0), Duration::from_secs(8));
/// assert_eq!(backoff.delay_with(9, 1.0), Duration::from_secs(30));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Backoff {
    /// The delay before the first retry.
    pub base: Duration,
    /// What each further attempt multiplies the delay by.
    pub factor: u32,
    /// The longest delay the curve may produce.
    pub maximum: Duration,
    /// The fraction of a delay that jitter may subtract, in `0.0..=1.0`.
    pub jitter: f64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(60),
            factor: 2,
            maximum: Duration::from_secs(3_600),
            jitter: Self::DEFAULT_JITTER,
        }
    }
}

impl Backoff {
    /// A quarter of a delay: wide enough to break up a fleet retrying in
    /// lockstep, narrow enough that the curve still reads as the one asked for.
    pub const DEFAULT_JITTER: f64 = 0.25;

    /// The delay before `attempt`, with jitter drawn at random.
    pub fn delay(&self, attempt: u32) -> Duration {
        self.delay_with(attempt, rand::random_range(0.0..1.0))
    }

    /// The delay before `attempt` for a given jitter draw, so the curve can be
    /// checked without a random number generator.
    ///
    /// `sample` is a fraction of [`Backoff::jitter`]; a value outside
    /// `0.0..=1.0`, or one that is not a number, is clamped.
    pub fn delay_with(&self, attempt: u32, sample: f64) -> Duration {
        let span = self
            .base
            .saturating_mul(self.factor.saturating_pow(attempt))
            .min(self.maximum);

        span.mul_f64(1.0 - fraction(self.jitter) * fraction(sample))
    }
}

fn fraction(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve() -> Backoff {
        Backoff {
            base: Duration::from_millis(100),
            factor: 2,
            maximum: Duration::from_millis(1_000),
            jitter: 0.0,
        }
    }

    #[test]
    fn the_first_attempt_waits_the_base_delay() {
        assert_eq!(curve().delay_with(0, 1.0), Duration::from_millis(100));
    }

    #[test]
    fn each_attempt_multiplies_by_the_factor() {
        let curve = curve();

        assert_eq!(curve.delay_with(1, 0.0), Duration::from_millis(200));
        assert_eq!(curve.delay_with(2, 0.0), Duration::from_millis(400));
        assert_eq!(curve.delay_with(3, 0.0), Duration::from_millis(800));
    }

    #[test]
    fn the_curve_is_capped_at_the_maximum() {
        let curve = curve();

        assert_eq!(curve.delay_with(4, 0.0), Duration::from_millis(1_000));
        assert_eq!(
            curve.delay_with(u32::MAX, 0.0),
            Duration::from_millis(1_000)
        );
    }

    #[test]
    fn the_default_curve_matches_a_minute_doubling_to_an_hour() {
        let curve = Backoff {
            jitter: 0.0,
            ..Backoff::default()
        };

        assert_eq!(curve.delay_with(0, 0.0), Duration::from_secs(60));
        assert_eq!(curve.delay_with(1, 0.0), Duration::from_secs(120));
        assert_eq!(curve.delay_with(6, 0.0), Duration::from_secs(3_600));
        assert_eq!(curve.delay_with(60, 0.0), Duration::from_secs(3_600));
    }

    #[test]
    fn jitter_only_ever_subtracts() {
        let curve = Backoff {
            jitter: 0.25,
            ..curve()
        };
        let span = Duration::from_millis(400);

        for sample in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let delay = curve.delay_with(2, sample);
            assert!(delay <= span, "{delay:?} outran the uncapped span");
            assert!(delay >= span.mul_f64(0.75), "{delay:?} undercut the jitter");
        }
    }

    #[test]
    fn the_smallest_draw_leaves_the_delay_alone() {
        let curve = Backoff {
            jitter: 1.0,
            ..curve()
        };

        assert_eq!(curve.delay_with(1, 0.0), Duration::from_millis(200));
        assert_eq!(curve.delay_with(1, 1.0), Duration::ZERO);
    }

    #[test]
    fn a_nonsense_jitter_or_draw_is_clamped_rather_than_panicking() {
        for jitter in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -4.0, 9.0] {
            let curve = Backoff { jitter, ..curve() };

            for sample in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 7.0] {
                let delay = curve.delay_with(2, sample);
                assert!(
                    delay <= Duration::from_millis(400),
                    "jitter {jitter} sample {sample} produced {delay:?}"
                );
            }
        }
    }

    #[test]
    fn a_zero_factor_collapses_the_curve_to_the_base_delay_once() {
        let curve = Backoff {
            factor: 0,
            ..curve()
        };

        assert_eq!(curve.delay_with(0, 0.0), Duration::from_millis(100));
        assert_eq!(curve.delay_with(1, 0.0), Duration::ZERO);
    }

    #[test]
    fn a_drawn_delay_obeys_the_same_bounds_as_a_sampled_one() {
        let curve = Backoff {
            jitter: 0.25,
            ..curve()
        };
        let span = Duration::from_millis(400);

        for _ in 0..256 {
            let delay = curve.delay(2);
            assert!(delay <= span, "{delay:?} escaped the curve");
            assert!(delay >= span.mul_f64(0.75), "{delay:?} undercut the jitter");
        }
    }
}
