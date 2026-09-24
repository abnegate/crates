use serde_json::Value;
use serde_json::json;

/// Parameter name, listed in both `properties` and `required`.
pub const REASON_PARAMETER: &str = "reason";

/// Written once here so no tool's copy of it can fork from another's.
///
/// Re-sent on seven schemas every turn, so it stays a bare reminder: when the
/// user reads it, and what a good one looks like, are said once in the file
/// section of the prompt instead of seven times here.
pub const REASON_DESCRIPTION: &str = "Why this call is needed, in one sentence. The user sees it.";

/// The schema fragment every side-effecting tool puts under [`REASON_PARAMETER`].
///
/// `required` is a lever on model behaviour, not a precondition: nothing
/// validates it at dispatch, so the matching field stays optional and its
/// absence never fails a call. Enforcement is human — the approval card shows
/// the reason, or shows that none was given.
pub fn reason_property() -> Value {
    json!({
        "type": "string",
        "description": REASON_DESCRIPTION
    })
}
