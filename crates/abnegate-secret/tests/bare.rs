//! How reading a value sealed before the envelope existed is reported.
//!
//! A `tracing` callsite caches its interest for the whole process, from the
//! subscriber of whichever thread reaches it first, so this file holds this
//! one test and nothing else that reads a bare body.

#![cfg(feature = "encryption")]

use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

use abnegate_secret::MasterKey;
use abnegate_secret::SecretValue;
use abnegate_secret::decrypt_legacy_value;
use abnegate_secret::encrypt_value;
use tracing::Event;
use tracing::Level;
use tracing::Metadata;
use tracing::Subscriber;
use tracing::field::Field;
use tracing::field::Visit;
use tracing::span;

const READS: usize = 3;
const SPAN: u64 = 1;
const ENVELOPE_PREFIX: &str = "ENC[v1:";
const ENVELOPE_SUFFIX: &str = "]";
const PLAINTEXT: &str = "sk-live-sealed-before-the-envelope";

struct Report {
    level: Level,
    fields: String,
}

struct FieldRecorder<'a> {
    fields: &'a mut String,
}

impl Visit for FieldRecorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.fields
            .push_str(&format!("{}={value:?};", field.name()));
    }
}

struct ReportRecorder {
    reports: Arc<Mutex<Vec<Report>>>,
}

impl Subscriber for ReportRecorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(SPAN)
    }

    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}

    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = String::new();
        event.record(&mut FieldRecorder {
            fields: &mut fields,
        });
        self.reports.lock().unwrap().push(Report {
            level: *event.metadata().level(),
            fields,
        });
    }

    fn enter(&self, _: &span::Id) {}

    fn exit(&self, _: &span::Id) {}
}

#[test]
fn reading_a_bare_body_is_reported_below_warning_level_without_its_value() {
    let reports = Arc::new(Mutex::new(Vec::new()));
    let recorder = ReportRecorder {
        reports: Arc::clone(&reports),
    };
    let key = MasterKey::generate().unwrap();
    let envelope = encrypt_value(&SecretValue::new(PLAINTEXT), &key).unwrap();
    let body = envelope
        .strip_prefix(ENVELOPE_PREFIX)
        .and_then(|rest| rest.strip_suffix(ENVELOPE_SUFFIX))
        .unwrap();

    tracing::subscriber::with_default(recorder, || {
        for _ in 0..READS {
            assert_eq!(
                decrypt_legacy_value(body, &key).unwrap().expose(),
                PLAINTEXT
            );
        }
        decrypt_legacy_value(&envelope, &key).unwrap();
    });

    let reports = reports.lock().unwrap();
    assert_eq!(reports.len(), READS, "an envelope read was reported");
    for report in reports.iter() {
        assert_eq!(report.level, Level::DEBUG);
        assert!(
            report.fields.contains("re-save it"),
            "the report does not ask for a re-save: {}",
            report.fields
        );
        assert!(
            !report.fields.contains(PLAINTEXT) && !report.fields.contains(body),
            "the report leaks the value: {}",
            report.fields
        );
    }
}
