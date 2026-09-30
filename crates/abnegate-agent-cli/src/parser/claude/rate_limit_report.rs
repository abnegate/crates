use serde::Deserialize;
use serde::Serialize;

const ALLOWED: [&str; 2] = ["allowed", "allowed_warning"];

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
}

#[cfg(test)]
mod tests {
    use super::RateLimitReport;

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
