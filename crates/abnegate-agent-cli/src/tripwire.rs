//! A caller's test for a diagnostic that means the run has already failed.

use std::fmt;
use std::sync::Arc;

/// Decides whether one line of an agent's stderr settles its run as failed.
///
/// It holds a closure rather than a function pointer, so the test can carry
/// state of its own, such as the patterns a caller loaded from its
/// configuration.
#[derive(Clone)]
pub struct Tripwire(Arc<dyn Fn(&str) -> bool + Send + Sync>);

impl Tripwire {
    pub fn new(test: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(test))
    }

    pub fn trips(&self, line: &str) -> bool {
        (self.0)(line)
    }
}

impl fmt::Debug for Tripwire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Tripwire")
    }
}

#[cfg(test)]
mod tests {
    use super::Tripwire;

    #[test]
    fn a_tripwire_can_carry_state_of_its_own() {
        let patterns = ["429".to_string(), "overloaded".to_string()];
        let tripwire =
            Tripwire::new(move |line| patterns.iter().any(|pattern| line.contains(pattern)));

        assert!(tripwire.trips("HTTP 429 Too Many Requests"));
        assert!(tripwire.clone().trips("the API is overloaded"));
        assert!(!tripwire.trips("compiling"));
        assert_eq!(format!("{tripwire:?}"), "Tripwire");
    }
}
