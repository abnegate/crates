use abnegate_llm::Window;
use abnegate_llm::WindowKind;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::parser::claude::lenient;

const ALLOWED: [&str; 2] = ["allowed", "allowed_warning"];

/// The window claude reports refused when an allowance that only usage
/// credits extend is spent. It is no part of the plan's usage.
pub(crate) const OVERAGE_INCLUDED_WINDOW: &str = "seven_day_overage_included";

/// claude reports a window's use as a fraction; a [`Window`] holds a percent.
const PERCENT: f64 = 100.0;

/// The throttling report nested inside a `rate_limit_event`.
///
/// It serialises back to the CLI's own field names, so a report quoted in a
/// failure still reads as the CLI wrote it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RateLimitReport {
    /// `allowed` or `allowed_warning` for headroom; anything else, or
    /// nothing, is a refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// A Unix timestamp or an RFC 3339 string, depending on the CLI release.
    #[serde(default, rename = "resetsAt", skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<serde_json::Value>,
    /// Which window was hit: `five_hour`, `seven_day`, and so on.
    #[serde(
        default,
        rename = "rateLimitType",
        skip_serializing_if = "Option::is_none"
    )]
    pub kind: Option<String>,
    /// How much of the window is spent, as a fraction where 1 is all of it.
    /// A value of any other shape reads as absent rather than losing the
    /// whole report.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub utilization: Option<f64>,
    /// Whether the account's usage credits carry the request past the
    /// window, which the CLI then reports as refused.
    #[serde(
        default,
        rename = "isUsingOverage",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_using_overage: Option<bool>,
    /// Whether the account may spend usage credits at all.
    #[serde(
        default,
        rename = "overageStatus",
        skip_serializing_if = "Option::is_none"
    )]
    pub overage_status: Option<String>,
    /// Why the account may not spend usage credits, such as
    /// `out_of_credits`, or `fetch_error` when the CLI could not look them up.
    #[serde(
        default,
        rename = "overageDisabledReason",
        skip_serializing_if = "Option::is_none"
    )]
    pub overage_disabled_reason: Option<String>,
    /// The server's code for a refusal the CLI reports on the line after
    /// this event, such as `credits_required`.
    #[serde(default, rename = "errorCode", skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

impl RateLimitReport {
    /// Whether this reports headroom rather than refusing the request: an
    /// explicit headroom status, or usage credits carrying the request past
    /// the window.
    ///
    /// A report with neither, one with no status at all included, is a
    /// refusal, since the event exists to announce one. Usage credits the
    /// account merely allows carry nothing past a window.
    pub fn allowed(&self) -> bool {
        self.is_using_overage == Some(true)
            || self
                .status
                .as_deref()
                .is_some_and(|status| ALLOWED.contains(&status))
    }

    /// When the window starts over, read from the report's `resetsAt`, which
    /// claude has written both in epoch seconds, as a number or a string, and
    /// as an RFC 3339 timestamp. Any other shape is `None`.
    pub fn resets_at(&self) -> Option<DateTime<Utc>> {
        self.resets_at.as_ref().and_then(reset_time)
    }

    /// The plan's window this report measures, when its status reports
    /// headroom and it says how much of the window is spent.
    pub(crate) fn measured(&self) -> Option<Window> {
        let headroom = self
            .status
            .as_deref()
            .is_some_and(|status| ALLOWED.contains(&status));
        if !headroom {
            return None;
        }
        self.window(self.fraction()? * PERCENT)
    }

    /// The window this report refused, counted as spent in full when it does
    /// not say how much of it was used.
    pub(crate) fn refused(&self) -> Option<Window> {
        self.window(
            self.fraction()
                .map_or(PERCENT, |fraction| fraction * PERCENT),
        )
    }

    /// How much of the window is spent, when the report says so as a
    /// fraction a window can hold: a use past the whole window, or below
    /// none, is read as unknown.
    fn fraction(&self) -> Option<f64> {
        self.utilization
            .filter(|fraction| (0.0..=1.0).contains(fraction))
    }

    /// The named window this report is on, `used_percent` spent, under the
    /// name claude streams it with. The window usage credits carry a turn
    /// past is none of the plan's.
    fn window(&self, used_percent: f64) -> Option<Window> {
        let name = self
            .kind
            .as_deref()
            .filter(|name| *name != OVERAGE_INCLUDED_WINDOW)?;
        let mut window = Window::new(name).with_used_percent(used_percent);
        if let Some(resets_at) = self.resets_at() {
            window = window.with_resets_at(resets_at);
        }
        if let Some(kind) = WindowKind::from_streamed(name) {
            window = window.with_kind(kind);
        }
        Some(window)
    }
}

fn reset_time(value: &Value) -> Option<DateTime<Utc>> {
    let seconds = match value {
        Value::Number(seconds) => seconds.as_i64(),
        Value::String(text) => match DateTime::parse_from_rfc3339(text) {
            Ok(time) => return Some(time.with_timezone(&Utc)),
            Err(_) => text.trim().parse().ok(),
        },
        _ => None,
    };
    seconds.and_then(|seconds| DateTime::from_timestamp(seconds, 0))
}

#[cfg(test)]
mod tests {
    use abnegate_llm::WindowKind;
    use chrono::DateTime;
    use serde_json::json;

    use super::RateLimitReport;

    fn report(value: serde_json::Value) -> RateLimitReport {
        serde_json::from_value(value).expect("a rate limit report")
    }

    #[test]
    fn a_reset_time_reads_from_seconds_or_a_timestamp_and_nothing_else() {
        let reset = DateTime::from_timestamp(1_790_222_400, 0);
        for (resets_at, expected) in [
            (json!(1_790_222_400), reset),
            (json!("1790222400"), reset),
            (json!(" 1790222400 "), reset),
            (json!("2026-09-24T04:00:00Z"), reset),
            (json!("2026-09-24T16:00:00+12:00"), reset),
            (json!(1.5), None),
            (json!("next Tuesday"), None),
            (json!(["2026-09-24T04:00:00Z"]), None),
            (serde_json::Value::Null, None),
        ] {
            let read = report(json!({"status": "rejected", "resetsAt": resets_at}));
            assert_eq!(read.resets_at(), expected, "{resets_at}");
        }
        assert_eq!(RateLimitReport::default().resets_at(), None);
    }

    #[test]
    fn a_use_of_another_shape_reads_as_unknown_and_keeps_the_report() {
        for utilization in [json!("0.5"), json!({"fraction": 0.5}), json!(true)] {
            let read = report(json!({
                "status": "rejected",
                "rateLimitType": "five_hour",
                "utilization": utilization,
            }));
            assert_eq!(read.utilization, None, "{utilization}");
            assert_eq!(read.kind.as_deref(), Some("five_hour"), "{utilization}");
        }
    }

    #[test]
    fn only_headroom_that_says_its_use_measures_a_window() {
        let measured = report(json!({
            "status": "allowed_warning",
            "rateLimitType": "seven_day_opus",
            "utilization": 0.81,
            "resetsAt": 1_790_208_000,
        }))
        .measured()
        .expect("a measured window");
        assert_eq!(measured.name, "seven_day_opus");
        assert_eq!(measured.kind, Some(WindowKind::SevenDaysOpus));
        assert!((measured.used_percent.expect("a use") - 81.0).abs() < 1e-9);
        assert_eq!(
            measured.resets_at,
            DateTime::from_timestamp(1_790_208_000, 0)
        );

        for unmeasured in [
            json!({"status": "allowed", "rateLimitType": "five_hour"}),
            json!({"status": "allowed", "utilization": 0.5}),
            json!({"status": "rejected", "rateLimitType": "five_hour", "utilization": 0.5}),
            json!({"rateLimitType": "five_hour", "utilization": 0.5, "isUsingOverage": true}),
            json!({"status": "allowed", "rateLimitType": "seven_day_overage_included", "utilization": 0.5}),
        ] {
            assert_eq!(report(unmeasured.clone()).measured(), None, "{unmeasured}");
        }
    }

    #[test]
    fn a_use_outside_the_window_is_never_reported_as_a_percentage() {
        for utilization in [1.8, -0.2, f64::INFINITY] {
            let headroom = report(json!({
                "status": "allowed",
                "rateLimitType": "five_hour",
                "utilization": utilization,
            }));
            assert_eq!(headroom.measured(), None, "{utilization}");

            let refused = report(json!({
                "status": "rejected",
                "rateLimitType": "five_hour",
                "utilization": utilization,
            }))
            .refused()
            .expect("a refused window");
            assert_eq!(refused.used_percent, Some(100.0), "{utilization}");
        }
    }

    #[test]
    fn a_refused_window_is_spent_in_full_unless_it_says_otherwise() {
        let refused = report(json!({"status": "rejected", "rateLimitType": "five_hour"}))
            .refused()
            .expect("a refused window");
        assert_eq!(refused.used_percent, Some(100.0));
        assert_eq!(refused.kind, Some(WindowKind::FiveHours));

        let partly = report(json!({
            "status": "rejected",
            "rateLimitType": "hourly_burst",
            "utilization": 0.9,
        }))
        .refused()
        .expect("a refused window");
        assert!((partly.used_percent.expect("a use") - 90.0).abs() < 1e-9);
        assert_eq!(partly.kind, None);

        for unnamed in [
            json!({"status": "rejected"}),
            json!({"status": "rejected", "rateLimitType": "seven_day_overage_included"}),
        ] {
            assert_eq!(report(unnamed.clone()).refused(), None, "{unnamed}");
        }
    }

    #[test]
    fn a_headroom_report_is_allowed() {
        let report: RateLimitReport = serde_json::from_str(
            r#"{"status":"allowed_warning","resetsAt":1772096400,"rateLimitType":"seven_day","utilization":0.81,"isUsingOverage":false,"surpassedThreshold":0.75}"#,
        )
        .expect("a rate limit report");

        assert!(report.allowed());
        assert_eq!(report.kind.as_deref(), Some("seven_day"));
        assert_eq!(report.resets_at, Some(serde_json::json!(1772096400)));
        assert!((report.utilization.expect("a utilisation") - 0.81).abs() < f64::EPSILON);
        assert_eq!(report.is_using_overage, Some(false));
    }

    #[test]
    fn a_report_serialises_under_the_clis_own_names() {
        let report = RateLimitReport {
            status: Some("rejected".to_string()),
            resets_at: Some(serde_json::json!("2026-02-23T06:00:00Z")),
            kind: Some("seven_day".to_string()),
            utilization: None,
            is_using_overage: Some(false),
            overage_status: Some("rejected".to_string()),
            overage_disabled_reason: Some("out_of_credits".to_string()),
            error_code: Some("credits_required".to_string()),
        };

        assert_eq!(
            serde_json::to_string(&report).expect("serialisable"),
            r#"{"status":"rejected","resetsAt":"2026-02-23T06:00:00Z","rateLimitType":"seven_day","isUsingOverage":false,"overageStatus":"rejected","overageDisabledReason":"out_of_credits","errorCode":"credits_required"}"#
        );
    }

    #[test]
    fn a_report_without_the_usage_credit_fields_serialises_without_them() {
        let report: RateLimitReport =
            serde_json::from_str(r#"{"status":"rejected","rateLimitType":"five_hour"}"#)
                .expect("a rate limit report");

        assert_eq!(
            serde_json::to_string(&report).expect("serialisable"),
            r#"{"status":"rejected","rateLimitType":"five_hour"}"#
        );
    }

    #[test]
    fn a_refusal_or_a_missing_status_is_not_allowed() {
        let exceeded: RateLimitReport = serde_json::from_str(
            r#"{"status":"exceeded","resetsAt":"2026-02-23T06:00:00Z","rateLimitType":"seven_day","utilization":1.0}"#,
        )
        .expect("a rate limit report");
        assert!(!exceeded.allowed());
        assert_eq!(
            exceeded.resets_at.as_ref().and_then(|value| value.as_str()),
            Some("2026-02-23T06:00:00Z")
        );

        assert!(!RateLimitReport::default().allowed());
    }

    #[test]
    fn a_refused_window_usage_credits_carry_the_request_past_is_allowed() {
        let carried: RateLimitReport = serde_json::from_str(
            r#"{"status":"rejected","rateLimitType":"five_hour","overageStatus":"allowed","isUsingOverage":true}"#,
        )
        .expect("a rate limit report");
        assert!(carried.allowed());

        for report in [
            r#"{"status":"rejected","rateLimitType":"five_hour","overageStatus":"allowed","isUsingOverage":false}"#,
            r#"{"status":"rejected","rateLimitType":"five_hour","overageStatus":"allowed"}"#,
        ] {
            let merely_allowed: RateLimitReport =
                serde_json::from_str(report).expect("a rate limit report");
            assert!(!merely_allowed.allowed(), "{report}");
        }
    }
}
