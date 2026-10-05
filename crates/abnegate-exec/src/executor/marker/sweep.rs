use std::collections::BTreeSet;
use std::io;

use crate::error::MarkerError;

/// What one or more passes over the marked processes reached: every process
/// a signal was delivered to, and the first failure.
#[derive(Debug, Default)]
pub(super) struct Sweep {
    signalled: BTreeSet<u32>,
    failure: Option<MarkerError>,
}

impl Sweep {
    /// Record a delivery to `pid`: whether it still carried the marker and
    /// took the signal, or why the signal was refused.
    pub(super) fn record(&mut self, pid: u32, delivery: io::Result<bool>) {
        match delivery {
            Ok(true) => {
                self.signalled.insert(pid);
            }
            Ok(false) => {}
            Err(source) => self.fail(MarkerError::SignalFailed { pid, source }),
        }
    }

    pub(super) fn fail(&mut self, error: MarkerError) {
        self.failure.get_or_insert(error);
    }

    /// The processes signalled, in ascending order, unless anything failed.
    pub(super) fn finish(self) -> Result<Vec<u32>, MarkerError> {
        match self.failure {
            Some(error) => Err(error),
            None => Ok(self.signalled.into_iter().collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_signalled_twice_is_reported_once() {
        let mut sweep = Sweep::default();

        sweep.record(7, Ok(true));
        sweep.record(3, Ok(true));
        sweep.record(7, Ok(true));
        sweep.record(5, Ok(false));

        assert_eq!(sweep.finish().unwrap(), vec![3, 7]);
    }

    #[test]
    fn the_first_failure_is_reported_after_the_rest_of_the_sweep() {
        let mut sweep = Sweep::default();

        sweep.record(3, Err(io::Error::from(io::ErrorKind::PermissionDenied)));
        sweep.record(4, Ok(true));
        sweep.fail(MarkerError::ListFailed(io::Error::from(
            io::ErrorKind::NotFound,
        )));

        assert!(matches!(
            sweep.finish(),
            Err(MarkerError::SignalFailed { pid: 3, .. })
        ));
    }
}
