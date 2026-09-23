use std::collections::HashSet;

use abnegate_llm::Message;
use abnegate_llm::Role;
use abnegate_llm::ToolDefinition;

use super::ContextBreakdown;
use super::ContextSource;
use super::ContextStatus;
use super::ContextUsage;
use super::Entry;
use super::Policy;
use super::Summary;
use super::compact::summary_message;

/// Tokens every message spends on framing, whatever it carries.
const MESSAGE_FRAMING_TOKENS: u64 = 8;

/// Tokens a request spends on framing before its first message.
pub(super) const REQUEST_FRAMING_TOKENS: u64 = 3;

/// Estimated tokens in `text`: a UTF-8 byte count, not a provider tokenizer
/// measurement.
///
/// Four bytes per token matches the common English heuristic; message framing
/// and [`Policy::threshold`]'s 20% input headroom remain the conservative
/// slack.
pub fn tokens(text: &str) -> u64 {
    u64::try_from(text.len()).unwrap_or(u64::MAX).div_ceil(4)
}

/// A message's estimated content tokens, and the tokens it spends around them.
pub(super) fn message_cost(message: &Message) -> (u64, u64) {
    let content = message.content.as_deref().map(tokens).unwrap_or_default();
    let mut overhead = MESSAGE_FRAMING_TOKENS;
    if let Some(name) = &message.name {
        overhead = overhead.saturating_add(tokens(name));
    }
    if let Some(id) = &message.tool_call_id {
        overhead = overhead.saturating_add(tokens(id));
    }
    if let Some(calls) = &message.tool_calls {
        overhead =
            overhead.saturating_add(tokens(&serde_json::to_string(calls).unwrap_or_default()));
    }
    if let Some(reasoning) = &message.reasoning_content {
        overhead = overhead.saturating_add(tokens(reasoning));
    }
    if !message.thinking_blocks.is_empty() {
        overhead = overhead.saturating_add(tokens(
            &serde_json::to_string(&message.thinking_blocks).unwrap_or_default(),
        ));
    }
    (content, overhead)
}

/// Estimate what sending `entries` and `tools` through `summary` would cost
/// against `policy`.
pub fn estimate(
    model: &str,
    entries: &[Entry],
    tools: Option<&[ToolDefinition]>,
    policy: &Policy,
    summary: Option<&Summary>,
) -> ContextUsage {
    let covered: HashSet<&str> = summary
        .map(|summary| {
            summary
                .coverage
                .entries
                .iter()
                .map(String::as_str)
                .collect()
        })
        .unwrap_or_default();
    let mut breakdown = ContextBreakdown {
        attachments: Some(0),
        overhead: REQUEST_FRAMING_TOKENS,
        ..Default::default()
    };
    for entry in entries
        .iter()
        .filter(|entry| !covered.contains(entry.id.as_str()))
    {
        let (content, overhead) = message_cost(&entry.message);
        let category = match entry.message.role {
            Role::System => &mut breakdown.instructions,
            Role::Tool => &mut breakdown.results,
            _ => &mut breakdown.conversation,
        };
        *category = category.saturating_add(content);
        breakdown.overhead = breakdown.overhead.saturating_add(overhead);
        if !entry.message.images.is_empty() || !entry.message.generated_images.is_empty() {
            breakdown.attachments = None;
        }
    }
    if let Some(summary) = summary {
        let (content, overhead) = message_cost(&summary_message(summary));
        breakdown.summary = content;
        breakdown.overhead = breakdown.overhead.saturating_add(overhead);
    }
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        breakdown.tools = tokens(&serde_json::to_string(tools).unwrap_or_default());
    }
    let used = breakdown.total();
    let threshold = policy.threshold();
    let incomplete = breakdown.attachments.is_none();
    let status = if policy.limit.is_none() {
        ContextStatus::Unavailable
    } else {
        ContextStatus::Ready
    };
    ContextUsage {
        model: model.into(),
        used,
        limit: policy.limit,
        reserved: policy.reserved,
        threshold,
        remaining: threshold.map(|threshold| threshold.saturating_sub(used)),
        estimated: true,
        incomplete,
        source: if policy.limit.is_none() {
            ContextSource::Unknown
        } else {
            policy.source
        },
        status,
        breakdown,
        revision: summary.map_or(0, |summary| summary.revision),
        compacted_messages: covered.len(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        reason: if policy.limit.is_none() {
            Some("Effective model context capacity is unavailable; automatic compaction cannot be budgeted.".into())
        } else if incomplete {
            Some("Image token costs are unavailable; known text costs are estimated.".into())
        } else {
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use abnegate_llm::Message;

    use super::MESSAGE_FRAMING_TOKENS;
    use super::message_cost;
    use super::tokens;

    /// The estimate rounds up: a partial token still costs a token. The
    /// claudear estimator this replaced rounded down, so a short message
    /// could cost nothing.
    #[test]
    fn a_partial_token_rounds_up() {
        assert_eq!(tokens("a"), 1);
        assert_eq!(tokens("abcde"), 2);
        assert_eq!(tokens("abcdefgh"), 2);
    }

    /// Every message spends eight tokens on framing, whatever it holds; the
    /// claudear estimator charged twenty. A name or a call id is charged on
    /// top, at the same four bytes a token.
    #[test]
    fn every_message_pays_the_same_framing() {
        assert_eq!(MESSAGE_FRAMING_TOKENS, 8);
        assert_eq!(message_cost(&Message::user("")), (0, 8));
        assert_eq!(message_cost(&Message::user("abcdefgh")), (2, 8));
        assert_eq!(message_cost(&Message::tool_result("abcd", "")), (0, 9));
    }

    #[test]
    fn four_utf8_bytes_are_one_token() {
        assert_eq!(tokens(""), 0);
        assert_eq!(tokens("abcd"), 1);
        assert_eq!(tokens("abcdefgh"), 2);
    }

    #[test]
    fn a_long_run_estimates_a_quarter_of_its_bytes() {
        assert_eq!(tokens(&"a".repeat(400)), 100);
    }
}
