//! How reading a value stored before the envelope existed is reported.
//!
//! A `tracing` callsite caches its interest for the whole process, and while
//! a single subscriber is registered that interest is taken from the
//! subscriber of whichever thread reaches the callsite first. A test on
//! another thread that read plaintext under no subscriber would cache "never"
//! for the recorder here, so this file holds this one test and nothing else
//! that reads plaintext.

#![cfg(feature = "encryption")]

use std::sync::Arc;
use std::sync::Mutex;

use abnegate_secret::MasterKey;
use abnegate_secret::decrypt_value;
use tracing::Event;
use tracing::Level;
use tracing::Metadata;
use tracing::Subscriber;
use tracing::span;

const READS: usize = 3;
const SPAN: u64 = 1;

struct LevelRecorder {
    levels: Arc<Mutex<Vec<Level>>>,
}

impl Subscriber for LevelRecorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(SPAN)
    }

    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}

    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        self.levels.lock().unwrap().push(*event.metadata().level());
    }

    fn enter(&self, _: &span::Id) {}

    fn exit(&self, _: &span::Id) {}
}

#[test]
fn reading_plaintext_is_reported_below_warning_level() {
    let levels = Arc::new(Mutex::new(Vec::new()));
    let recorder = LevelRecorder {
        levels: Arc::clone(&levels),
    };
    let key = MasterKey::generate().unwrap();

    tracing::subscriber::with_default(recorder, || {
        for _ in 0..READS {
            decrypt_value("written-before-the-envelope-existed", &key).unwrap();
        }
    });

    let levels = levels.lock().unwrap();
    assert_eq!(levels.as_slice(), [Level::DEBUG; READS]);
}
