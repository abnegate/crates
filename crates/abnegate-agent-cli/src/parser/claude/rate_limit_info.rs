use serde::Deserialize;

const ALLOWED: [&str; 2] = ["allowed", "allowed_warning"];

/// The throttling report nested inside a `rate_limit_event`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RateLimitInfo {
    #[serde(default)]
    pub status: Option<String>,
    /// A Unix timestamp or an RFC 3339 string, depending on the CLI release.
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<serde_json::Value>,
    /// Which window was hit: `five_hour`, `seven_day`, and so on.
    #[serde(default, rename = "rateLimitType")]
    pub kind: Option<String>,
    #[serde(default)]
    pub utilization: Option<f64>,
}

impl RateLimitInfo {
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
    use super::RateLimitInfo;

    #[test]
    fn a_headroom_report_is_allowed() {
        let info: RateLimitInfo = serde_json::from_str(
            r#"{"status":"allowed_warning","resetsAt":1772096400,"rateLimitType":"seven_day","utilization":0.81,"isUsingOverage":false,"surpassedThreshold":0.75}"#,
        )
        .expect("a rate limit report");

        assert!(info.allowed());
        assert_eq!(info.kind.as_deref(), Some("seven_day"));
        assert_eq!(info.resets_at, Some(serde_json::json!(1772096400)));
        assert!((info.utilization.expect("a utilisation") - 0.81).abs() < f64::EPSILON);
    }

    #[test]
    fn a_refusal_or_a_missing_status_is_not_allowed() {
        let exceeded: RateLimitInfo = serde_json::from_str(
            r#"{"status":"exceeded","resetsAt":"2026-02-23T06:00:00Z","rateLimitType":"seven_day","utilization":1.0}"#,
        )
        .expect("a rate limit report");
        assert!(!exceeded.allowed());
        assert_eq!(
            exceeded.resets_at.as_ref().and_then(|value| value.as_str()),
            Some("2026-02-23T06:00:00Z")
        );

        assert!(!RateLimitInfo::default().allowed());
    }
}
