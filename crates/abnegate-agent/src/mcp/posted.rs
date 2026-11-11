use std::sync::Mutex;
use std::sync::PoisonError;

use super::McpFailure;

/// What became of the last request one remote server was sent that a
/// connecting session waits on: why it failed, when it did, and the status
/// of the answer that failed it.
///
/// A [`Remote`](super::remote::Remote) keeps one behind a lock it shares
/// with the session, taken from each error before that error is rewritten
/// for showing, so the session can still name a failure the transport
/// handed it as text, or only as a connection that closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Posted {
    pub(super) failure: Option<McpFailure>,
    pub(super) status: Option<u16>,
}

impl Posted {
    /// What `slot` holds now.
    pub(super) fn read(slot: &Mutex<Self>) -> Self {
        *slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Put `self` in `slot`, over what it held.
    pub(super) fn write(self, slot: &Mutex<Self>) {
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = self;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_posted_names_no_failure() {
        assert_eq!(
            Posted::default(),
            Posted {
                failure: None,
                status: None
            }
        );
    }

    #[test]
    fn a_slot_reads_what_was_last_written() {
        let slot = Mutex::new(Posted::default());
        let failed = Posted {
            failure: Some(McpFailure::Status),
            status: Some(503),
        };

        failed.write(&slot);
        assert_eq!(Posted::read(&slot), failed);

        Posted::default().write(&slot);
        assert_eq!(Posted::read(&slot), Posted::default());
    }

    #[test]
    fn a_slot_poisoned_by_a_panic_still_reads_and_writes() {
        let slot = Mutex::new(Posted::default());
        let poisoned = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _held = slot.lock().expect("the first lock");
                    panic!("poison the slot");
                })
                .join()
        });
        assert!(poisoned.is_err() && slot.is_poisoned());
        let failed = Posted {
            failure: Some(McpFailure::Closed),
            status: None,
        };

        failed.write(&slot);

        assert_eq!(Posted::read(&slot), failed);
    }
}
