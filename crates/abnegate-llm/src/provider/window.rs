use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::provider::window_kind::WindowKind;

/// How much of one usage window a coding agent's plan has spent.
///
/// An application may store windows as JSON, so the serialized field names
/// are a stored format: `name`, `used_percent`, `used`, `limit` and
/// `resets_at`. [`Self::kind`] is never serialized, so a stored row reads
/// back with no kind and a window with one writes the same JSON as before.
///
/// Equality compares the stored fields and ignores `kind`, which is derived
/// from `name` when parsed, so a window equals itself read back from JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Window {
    /// The name the window is recorded under.
    pub name: String,
    /// How much of the window is spent, from 0 to 100.
    pub used_percent: Option<f64>,
    /// How many units of the window are spent.
    pub used: Option<u64>,
    /// How many units the window holds.
    pub limit: Option<u64>,
    /// When the window starts over.
    pub resets_at: Option<DateTime<Utc>>,
    /// Which of the plan's windows this is, when the agent said.
    #[serde(skip)]
    pub kind: Option<WindowKind>,
}

impl Window {
    /// A window recorded under `name`, with nothing yet known of its spend.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            used_percent: None,
            used: None,
            limit: None,
            resets_at: None,
            kind: None,
        }
    }

    /// The same window, `used_percent` spent.
    pub fn with_used_percent(mut self, used_percent: f64) -> Self {
        self.used_percent = Some(used_percent);
        self
    }

    /// The same window, `used` units spent.
    pub fn with_used(mut self, used: u64) -> Self {
        self.used = Some(used);
        self
    }

    /// The same window, holding `limit` units.
    pub fn with_limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// The same window, starting over at `resets_at`.
    pub fn with_resets_at(mut self, resets_at: DateTime<Utc>) -> Self {
        self.resets_at = Some(resets_at);
        self
    }

    /// The same window, known to be the plan's `kind` window.
    pub fn with_kind(mut self, kind: WindowKind) -> Self {
        self.kind = Some(kind);
        self
    }
}

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            name,
            used_percent,
            used,
            limit,
            resets_at,
            kind: _,
        } = self;
        *name == other.name
            && *used_percent == other.used_percent
            && *used == other.used
            && *limit == other.limit
            && *resets_at == other.resets_at
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use chrono::Utc;

    use super::Window;
    use crate::provider::window_kind::WindowKind;

    const STORED: &str = r#"{"name":"5h","used_percent":42.0,"used":null,"limit":null,"resets_at":"2026-10-05T00:00:00Z"}"#;

    fn midnight() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-05T00:00:00Z")
            .expect("a valid timestamp")
            .with_timezone(&Utc)
    }

    fn stored() -> Window {
        Window::new("5h")
            .with_used_percent(42.0)
            .with_resets_at(midnight())
    }

    #[test]
    fn a_window_writes_the_stored_field_names() {
        assert_eq!(
            serde_json::to_string(&stored()).expect("a window to serialize"),
            STORED
        );
    }

    #[test]
    fn a_window_with_a_kind_writes_the_same_stored_row() {
        let window = stored().with_kind(WindowKind::FiveHours);

        assert_eq!(
            serde_json::to_string(&window).expect("a window to serialize"),
            STORED
        );
    }

    #[test]
    fn a_stored_row_without_a_kind_still_reads() {
        let window: Window = serde_json::from_str(STORED).expect("a stored row to read");

        assert_eq!(window, stored());
        assert_eq!(window.kind, None);
    }

    #[test]
    fn a_window_with_a_kind_equals_itself_read_back_from_json() {
        let window = stored().with_kind(WindowKind::FiveHours);

        let read: Window =
            serde_json::from_str(&serde_json::to_string(&window).expect("a window to serialize"))
                .expect("a stored row to read");

        assert_eq!(read, window);
        assert_eq!(read.kind, None);
    }

    #[test]
    fn windows_that_differ_in_a_stored_field_are_unequal() {
        let window = stored().with_kind(WindowKind::FiveHours);

        assert_ne!(window, stored().with_used_percent(43.0));
        assert_ne!(window, stored().with_used(1));
        assert_ne!(window, stored().with_limit(100));
        assert_ne!(
            window,
            Window::new("7d")
                .with_used_percent(42.0)
                .with_resets_at(midnight())
        );
        assert_ne!(window, Window::new("5h").with_used_percent(42.0));
    }

    #[test]
    fn a_stored_row_missing_its_counters_reads_them_as_unknown() {
        let window: Window = serde_json::from_str(r#"{"name":"7d Opus"}"#).expect("a sparse row");

        assert_eq!(window, Window::new("7d Opus"));
    }

    #[test]
    fn every_builder_sets_its_own_field() {
        let window = Window::new("seven_day")
            .with_used(30)
            .with_limit(100)
            .with_kind(WindowKind::SevenDays);

        assert_eq!(window.name, "seven_day");
        assert_eq!(window.used, Some(30));
        assert_eq!(window.limit, Some(100));
        assert_eq!(window.kind, Some(WindowKind::SevenDays));
        assert_eq!(window.used_percent, None);
        assert_eq!(window.resets_at, None);
    }
}
