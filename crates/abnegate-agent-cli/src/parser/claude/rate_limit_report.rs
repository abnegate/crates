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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization: Option<f64>,
}

impl RateLimitReport {
    /// Whether this reports headroom rather than refusing the request.
    ///
    /// Only an explicit headroom status counts. A report with no status at
    /// all is treated as a refusal, since the event exists to announce one.
    pub fn allowed(&self) -> bool {
        self.status
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
    }

    #[test]
    fn a_report_serialises_under_the_clis_own_names() {
        let report = RateLimitReport {
            status: Some("rejected".to_string()),
            resets_at: Some(serde_json::json!("2026-02-23T06:00:00Z")),
            kind: Some("seven_day".to_string()),
            utilization: None,
        };

        assert_eq!(
            serde_json::to_string(&report).expect("serialisable"),
            r#"{"status":"rejected","resetsAt":"2026-02-23T06:00:00Z","rateLimitType":"seven_day"}"#
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
}
