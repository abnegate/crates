use std::fmt;

use serde::Deserialize;
use serde::Deserializer;
use serde::de::IgnoredAny;
use serde::de::MapAccess;
use serde::de::SeqAccess;
use serde::de::Visitor;
use serde::de::value::MapAccessDeserializer;
use serde_json::Value;

use crate::parser::lenient;

/// The `tool_use_result` of a call that started a subagent: its report.
///
/// Every tool writes an account of its call there in a shape of its own, so
/// only an object is read as one, field by field and leniently, and every
/// other field, the subagent's whole answer among them, is skipped unread.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Report {
    #[serde(deserialize_with = "lenient")]
    pub(crate) status: Option<String>,
    #[serde(rename = "resolvedModel", deserialize_with = "lenient")]
    pub(crate) model: Option<String>,
    #[serde(rename = "totalToolUseCount", deserialize_with = "lenient")]
    pub(crate) tool_uses: Option<u64>,
    #[serde(rename = "totalDurationMs", deserialize_with = "lenient")]
    pub(crate) duration_ms: Option<u64>,
    #[serde(rename = "totalTokens", deserialize_with = "lenient")]
    pub(crate) tokens: Option<u64>,
    /// The id of the task claude runs the subagent as.
    #[serde(rename = "agentId", deserialize_with = "lenient")]
    pub(crate) task: Option<String>,
}

impl Report {
    /// The report `account` holds, when it is an object.
    pub(crate) fn from_value(account: &Value) -> Option<Self> {
        account
            .is_object()
            .then(|| Self::deserialize(account).ok())
            .flatten()
    }
}

/// A tool's account of its call, read as a [`Report`] when it is an object
/// and skipped unread, whatever else it is.
pub(crate) fn account<'de, D>(deserializer: D) -> Result<Option<Report>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(Account)
}

struct Account;

impl<'de> Visitor<'de> for Account {
    type Value = Option<Report>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a tool's account of its call")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        Report::deserialize(MapAccessDeserializer::new(map)).map(Some)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::Report;
    use super::account;

    #[derive(Debug, Deserialize)]
    struct Line {
        #[serde(default, deserialize_with = "account")]
        tool_use_result: Option<Report>,
        #[serde(default)]
        after: Option<String>,
    }

    fn read(line: &str) -> Line {
        serde_json::from_str(line).expect("a line")
    }

    #[test]
    fn an_object_is_read_as_a_report_field_by_field() {
        let line = read(
            r#"{"tool_use_result":{"status":"completed","agentId":"a1","resolvedModel":"claude-haiku-4-5","totalToolUseCount":2,"totalDurationMs":"slow","totalTokens":18200,"content":[{"type":"text","text":"long"}]},"after":"read on"}"#,
        );

        let report = line.tool_use_result.expect("a report");
        assert_eq!(report.status.as_deref(), Some("completed"));
        assert_eq!(report.task.as_deref(), Some("a1"));
        assert_eq!(report.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(report.tool_uses, Some(2));
        assert_eq!(report.duration_ms, None);
        assert_eq!(report.tokens, Some(18_200));
        assert_eq!(line.after.as_deref(), Some("read on"));
    }

    #[test]
    fn an_account_of_any_other_shape_is_no_report_and_costs_nothing_after_it() {
        for account in [
            r#""Error: File does not exist.""#,
            r#"[1,"two",{"three":3},[4]]"#,
            "null",
            "true",
            "-7",
            "7",
            "1.5",
        ] {
            let line = read(&format!(
                r#"{{"tool_use_result":{account},"after":"read on"}}"#
            ));
            assert!(line.tool_use_result.is_none(), "{account}");
            assert_eq!(line.after.as_deref(), Some("read on"), "{account}");
        }
        assert!(read("{}").tool_use_result.is_none());
    }

    #[test]
    fn a_report_is_read_from_a_value_only_when_it_is_an_object() {
        assert_eq!(
            Report::from_value(&json!({"status": "async_launched"}))
                .and_then(|report| report.status),
            Some("async_launched".to_string())
        );
        assert!(Report::from_value(&json!("Launched.")).is_none());
        assert!(Report::from_value(&json!([{"status": "completed"}])).is_none());
    }
}
