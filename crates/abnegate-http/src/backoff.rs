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
/// let backoff = Backoff::new(Duration::from_secs(1), 2, Duration::from_secs(30), 0.0);
///
/// assert_eq!(backoff.delay_with(0, 1.0), Duration::from_secs(1));
/// assert_eq!(backoff.delay_with(3, 1.0), Duration::from_secs(8));
/// assert_eq!(backoff.delay_with(9, 1.0), Duration::from_secs(30));
///
/// let sooner = Backoff::default().with_base(Duration::from_secs(1));
/// assert_eq!(sooner.maximum, Backoff::default().maximum);
/// ```
///
/// A field may be added in a minor release, so outside this crate a curve is
/// built with [`Backoff::new`] or adjusted from [`Backoff::default`] with the
/// `with_*` methods, never written out as a literal:
///
/// ```compile_fail,E0639
/// use abnegate_http::Backoff;
///
/// let _ = Backoff {
///     jitter: 0.0,
///     ..Backoff::default()
/// };
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
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
    /// A minute, doubling to at most an hour, with [`Backoff::DEFAULT_JITTER`].
    fn default() -> Self {
        Self::new(
            Duration::from_secs(60),
            2,
            Duration::from_secs(3_600),
            Self::DEFAULT_JITTER,
        )
    }
}

impl Backoff {
    /// A quarter of a delay: wide enough to break up a fleet retrying in
    /// lockstep, narrow enough that the curve still reads as the one asked for.
    pub const DEFAULT_JITTER: f64 = 0.25;

    /// A curve that waits `base` before the first retry, multiplies the delay
    /// by `factor` for each attempt after that, never waits longer than
    /// `maximum`, and subtracts up to the fraction `jitter` of each delay at
    /// random.
    pub const fn new(base: Duration, factor: u32, maximum: Duration, jitter: f64) -> Self {
        Self {
            base,
            factor,
            maximum,
            jitter,
        }
    }

    /// This curve, waiting `base` before the first retry.
    pub const fn with_base(self, base: Duration) -> Self {
        Self { base, ..self }
    }

    /// This curve, multiplying the delay by `factor` for each further attempt.
    pub const fn with_factor(self, factor: u32) -> Self {
        Self { factor, ..self }
    }

    /// This curve, never waiting longer than `maximum`.
    pub const fn with_maximum(self, maximum: Duration) -> Self {
        Self { maximum, ..self }
    }

    /// This curve, subtracting up to the fraction `jitter` of each delay at
    /// random.
    pub const fn with_jitter(self, jitter: f64) -> Self {
        Self { jitter, ..self }
    }

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
        Backoff::new(
            Duration::from_millis(100),
            2,
            Duration::from_millis(1_000),
            0.0,
        )
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
        let curve = Backoff::default().with_jitter(0.0);

        assert_eq!(curve.delay_with(0, 0.0), Duration::from_secs(60));
        assert_eq!(curve.delay_with(1, 0.0), Duration::from_secs(120));
        assert_eq!(curve.delay_with(6, 0.0), Duration::from_secs(3_600));
        assert_eq!(curve.delay_with(60, 0.0), Duration::from_secs(3_600));
    }

    #[test]
    fn jitter_only_ever_subtracts() {
        let curve = curve().with_jitter(0.25);
        let span = Duration::from_millis(400);

        for sample in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let delay = curve.delay_with(2, sample);
            assert!(delay <= span, "{delay:?} outran the uncapped span");
            assert!(delay >= span.mul_f64(0.75), "{delay:?} undercut the jitter");
        }
    }

    #[test]
    fn the_smallest_draw_leaves_the_delay_alone() {
        let curve = curve().with_jitter(1.0);

        assert_eq!(curve.delay_with(1, 0.0), Duration::from_millis(200));
        assert_eq!(curve.delay_with(1, 1.0), Duration::ZERO);
    }

    #[test]
    fn a_nonsense_jitter_or_draw_is_clamped_rather_than_panicking() {
        for jitter in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -4.0, 9.0] {
            let curve = curve().with_jitter(jitter);

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
        let curve = curve().with_factor(0);

        assert_eq!(curve.delay_with(0, 0.0), Duration::from_millis(100));
        assert_eq!(curve.delay_with(1, 0.0), Duration::ZERO);
    }

    #[test]
    fn a_drawn_delay_obeys_the_same_bounds_as_a_sampled_one() {
        let curve = curve().with_jitter(0.25);
        let span = Duration::from_millis(400);

        for _ in 0..256 {
            let delay = curve.delay(2);
            assert!(delay <= span, "{delay:?} escaped the curve");
            assert!(delay >= span.mul_f64(0.75), "{delay:?} undercut the jitter");
        }
    }

    #[test]
    fn a_curve_holds_the_values_it_was_built_with() {
        let curve = Backoff::new(Duration::from_millis(250), 3, Duration::from_secs(9), 0.5);

        assert_eq!(curve.base, Duration::from_millis(250));
        assert_eq!(curve.factor, 3);
        assert_eq!(curve.maximum, Duration::from_secs(9));
        assert_eq!(curve.jitter, 0.5);
    }

    #[test]
    fn each_with_method_replaces_only_its_own_value() {
        let original = curve();
        let maximum = Duration::from_millis(1_000);

        assert_eq!(
            original.with_base(Duration::from_millis(7)),
            Backoff::new(Duration::from_millis(7), 2, maximum, 0.0)
        );
        assert_eq!(
            original.with_factor(5),
            Backoff::new(Duration::from_millis(100), 5, maximum, 0.0)
        );
        assert_eq!(
            original.with_maximum(Duration::from_millis(9)),
            Backoff::new(Duration::from_millis(100), 2, Duration::from_millis(9), 0.0)
        );
        assert_eq!(
            original.with_jitter(0.5),
            Backoff::new(Duration::from_millis(100), 2, maximum, 0.5)
        );
    }

    #[test]
    fn the_default_curve_draws_the_default_jitter() {
        assert_eq!(Backoff::default().jitter, Backoff::DEFAULT_JITTER);
    }
}
