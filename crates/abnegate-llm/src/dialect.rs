use std::borrow::Cow;

use serde_json::Map;
use serde_json::Value;

use crate::budget::Budget;
use crate::modality::ResponseFormat;
use crate::wire::Message;
use crate::wire::Role;
use crate::wire::ToolDefinition;

/// Tokens that end an assistant turn across common chat templates.
pub const TEMPLATE_STOPS: &[&str] = &[
    "<|im_end|>",
    "<|im_start|>",
    "<|eot_id|>",
    "<|end_of_turn|>",
    "<|endoftext|>",
    "<|end_of_text|>",
];

const OPENAI_STOP_LIMIT: usize = 4;
const OPENAI_REASONING_FAMILIES: &[&str] = &["o1", "o3", "o4", "gpt-5"];
const OPENAI_FAMILY_SEPARATORS: &[char] = &['-', '.'];
const OPENAI_CONVERSATIONAL_VARIANTS: &[&str] = &["-chat"];
const ANTHROPIC_LOWEST_TEMPERATURE: f32 = 0.0;
const ANTHROPIC_HIGHEST_TEMPERATURE: f32 = 1.0;
const TOOL_IMAGE: &str = "[Image from the previous tool result]";
const ASSISTANT_IMAGE: &str = "[Image from the previous assistant message]";
const SCHEMA_TYPE: &str = "type";
const SCHEMA_PROPERTIES: &str = "properties";
const OBJECT_SCHEMA: &str = "object";

/// Which API sits behind [`LlmConfig::base_url`](crate::LlmConfig::base_url),
/// and so which request shape it accepts.
///
/// A gateway such as LiteLLM absorbs every provider's quirks, so a request
/// sent through it can carry whatever the caller asks for. A provider's own
/// API refuses what it does not support with a 400, so a request sent there
/// directly is shaped first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Dialect {
    /// LiteLLM, Ollama, vLLM or any other server that takes the request as is.
    #[default]
    Compatible,
    /// OpenAI's own API.
    OpenAI,
    /// Anthropic's OpenAI SDK compatibility layer.
    Anthropic,
}

impl Dialect {
    /// Whether `model` is an OpenAI reasoning model, which refuses `stop`, a
    /// non-default `temperature` and `max_tokens`. A family's `-chat`
    /// variant, such as `gpt-5-chat-latest`, does not reason.
    pub fn reasons(self, model: &str) -> bool {
        if self != Self::OpenAI {
            return false;
        }
        let name = model
            .rsplit('/')
            .next()
            .unwrap_or(model)
            .to_ascii_lowercase();
        let family = OPENAI_REASONING_FAMILIES.iter().any(|family| {
            name.strip_prefix(family)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(OPENAI_FAMILY_SEPARATORS))
        });
        family
            && !OPENAI_CONVERSATIONAL_VARIANTS
                .iter()
                .any(|variant| name.contains(variant))
    }

    /// The stops `model` accepts out of `stops`.
    ///
    /// OpenAI takes at most four, so the template tokens its own models never
    /// emit are dropped first; the caller's output filter still halts on them.
    /// Anthropic refuses a stop that is only whitespace. A compatible endpoint
    /// is handed the stops it was given, without a copy.
    pub fn stops<'a>(self, model: &str, stops: &'a [String]) -> Cow<'a, [String]> {
        match self {
            Self::Compatible => Cow::Borrowed(stops),
            Self::OpenAI if self.reasons(model) => Cow::Borrowed(&[]),
            Self::OpenAI => stops
                .iter()
                .filter(|stop| !stop.is_empty() && !TEMPLATE_STOPS.contains(&stop.as_str()))
                .take(OPENAI_STOP_LIMIT)
                .cloned()
                .collect(),
            Self::Anthropic => stops
                .iter()
                .filter(|stop| !stop.trim().is_empty())
                .cloned()
                .collect(),
        }
    }

    /// The temperature `model` accepts in place of `temperature`, or `None`
    /// when it takes only its own. Anthropic holds it between zero and one.
    pub fn temperature(self, model: &str, temperature: f32) -> Option<f32> {
        match self {
            Self::Compatible => Some(temperature),
            Self::OpenAI if self.reasons(model) => None,
            Self::OpenAI => Some(temperature),
            Self::Anthropic => {
                Some(temperature.clamp(ANTHROPIC_LOWEST_TEMPERATURE, ANTHROPIC_HIGHEST_TEMPERATURE))
            }
        }
    }

    /// The field `model` reads its output budget from.
    pub fn budget(self, model: &str) -> Budget {
        if self.reasons(model) {
            Budget::MaximumCompletionTokens
        } else {
            Budget::MaximumTokens
        }
    }

    /// Whether `reasoning_effort` may be sent for `model`. OpenAI refuses it
    /// on a model that does not reason, and Anthropic ignores it.
    pub fn effort(self, model: &str) -> bool {
        match self {
            Self::Compatible => true,
            Self::OpenAI => self.reasons(model),
            Self::Anthropic => false,
        }
    }

    /// Whether fields no provider API defines, such as Ollama's `num_ctx`,
    /// may be sent.
    pub fn extended(self) -> bool {
        self == Self::Compatible
    }

    /// `messages` as a provider's own API accepts them.
    ///
    /// OpenAI and Anthropic take image parts only from a user, so an image a
    /// tool or the assistant produced is moved into a user message of its
    /// own. One from a tool waits until the run of tool results ends, since
    /// nothing may come between a call and its results. Replayed reasoning
    /// is dropped: neither API defines it on a request message. A compatible
    /// endpoint is handed the history it was given, without a copy.
    pub fn messages(self, messages: &[Message]) -> Cow<'_, [Message]> {
        if self == Self::Compatible {
            return Cow::Borrowed(messages);
        }
        let mut shaped = Vec::with_capacity(messages.len());
        let mut forwarded = Vec::new();
        for message in messages {
            if message.role != Role::Tool {
                shaped.append(&mut forwarded);
            }
            let (kept, images) = separated(message);
            shaped.extend(kept);
            forwarded.extend(images);
        }
        shaped.append(&mut forwarded);
        Cow::Owned(shaped)
    }

    /// `format` as the provider's API accepts it. Anthropic accepts only a
    /// [strict](ResponseFormat::strict_json_schema) JSON schema, so any other
    /// format, a schema written only as guidance among them, is left to the
    /// prompt rather than refused with a 400.
    pub fn response_format(self, format: Option<&ResponseFormat>) -> Option<&ResponseFormat> {
        let format = format?;
        match (self, format) {
            (
                Self::Anthropic,
                ResponseFormat::Json {
                    schema: Some(_),
                    strict: true,
                },
            ) => Some(format),
            (Self::Anthropic, _) => None,
            _ => Some(format),
        }
    }

    /// `tools` as a provider's own API accepts them: never an empty list, and
    /// a function whose parameters are missing, `null` or an object without a
    /// `type` given `"type": "object"`. Parameters of any other shape, such
    /// as `{"type": "string"}`, are the caller's schema and are sent as given.
    /// A list that already complies is borrowed rather than copied.
    pub fn tools(self, tools: Option<&[ToolDefinition]>) -> Option<Cow<'_, [ToolDefinition]>> {
        let tools = tools?;
        if self == Self::Compatible {
            return Some(Cow::Borrowed(tools));
        }
        if tools.is_empty() {
            return None;
        }
        if tools.iter().all(|tool| typed(&tool.function.parameters)) {
            return Some(Cow::Borrowed(tools));
        }
        Some(Cow::Owned(
            tools
                .iter()
                .cloned()
                .map(|mut tool| {
                    tool.function.parameters = object(tool.function.parameters);
                    tool
                })
                .collect(),
        ))
    }
}

/// `message` without what a provider refuses on its role, and the user
/// message that carries its images instead.
fn separated(message: &Message) -> (Option<Message>, Option<Message>) {
    let mut kept = message.clone();
    kept.reasoning_content = None;
    kept.thinking_blocks = Vec::new();
    if kept.role == Role::Tool {
        kept.name = None;
    }
    let forwarded = (kept.role != Role::User && !kept.images.is_empty()).then(|| {
        let mut forwarded = Message::user(label(&kept));
        forwarded.images = std::mem::take(&mut kept.images);
        forwarded
    });
    if kept.role == Role::Assistant
        && kept
            .content
            .as_deref()
            .is_none_or(|content| content.trim().is_empty())
    {
        if kept.tool_calls.is_none() {
            return (None, forwarded);
        }
        kept.content = None;
    }
    (Some(kept), forwarded)
}

fn label(message: &Message) -> String {
    match (message.role, &message.tool_call_id) {
        (Role::Tool, Some(id)) => format!("[Image from the tool result for {id}]"),
        (Role::Tool, None) => TOOL_IMAGE.to_string(),
        _ => ASSISTANT_IMAGE.to_string(),
    }
}

fn typed(parameters: &Value) -> bool {
    parameters.get(SCHEMA_TYPE).is_some()
}

fn object(parameters: Value) -> Value {
    let mut schema = match parameters {
        Value::Object(schema) if schema.contains_key(SCHEMA_TYPE) => return Value::Object(schema),
        Value::Object(schema) => schema,
        Value::Null => Map::new(),
        other => return other,
    };
    schema.insert(SCHEMA_TYPE.to_string(), OBJECT_SCHEMA.into());
    schema
        .entry(SCHEMA_PROPERTIES)
        .or_insert_with(|| Value::Object(Map::new()));
    Value::Object(schema)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use serde_json::Value;

    use super::ASSISTANT_IMAGE;
    use super::Dialect;
    use super::TEMPLATE_STOPS;
    use super::TOOL_IMAGE;
    use crate::budget::Budget;
    use crate::wire::Message;
    use crate::wire::Role;
    use crate::wire::ToolCall;
    use crate::wire::ToolDefinition;

    const SCREENSHOT: &str = "data:image/png;base64,c2NyZWVu";

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn call(id: &str) -> ToolCall {
        ToolCall::function(id, "screenshot", "{}")
    }

    fn roles(messages: &[Message]) -> Vec<(Role, Option<&str>, usize)> {
        messages
            .iter()
            .map(|message| {
                (
                    message.role,
                    message.content.as_deref(),
                    message.images.len(),
                )
            })
            .collect()
    }

    #[test]
    fn openai_reasoning_models_are_recognised_by_prefix() {
        for model in [
            "o1",
            "o1-mini",
            "o3-mini",
            "o4-mini",
            "gpt-5",
            "gpt-5-mini",
            "openai/o3",
            "O3",
        ] {
            assert!(Dialect::OpenAI.reasons(model), "{model}");
        }
        for model in ["gpt-4o", "gpt-4.1-mini", "chatgpt-4o-latest"] {
            assert!(!Dialect::OpenAI.reasons(model), "{model}");
        }
        assert!(!Dialect::Compatible.reasons("o3"));
        assert!(!Dialect::Anthropic.reasons("o3"));
    }

    #[test]
    fn openai_chat_variants_and_lookalike_names_do_not_reason() {
        for model in [
            "gpt-5-chat-latest",
            "gpt-5.1-chat-latest",
            "openai/gpt-5-chat-latest",
            "GPT-5-CHAT-LATEST",
            "gpt-50",
            "o10-preview",
        ] {
            assert!(!Dialect::OpenAI.reasons(model), "{model}");
            assert_eq!(
                Dialect::OpenAI.budget(model),
                Budget::MaximumTokens,
                "{model}"
            );
            assert_eq!(
                Dialect::OpenAI.temperature(model, 0.2),
                Some(0.2),
                "{model}"
            );
        }
        for model in ["gpt-5.1", "gpt-5-codex", "gpt-5-nano", "o3-pro"] {
            assert!(Dialect::OpenAI.reasons(model), "{model}");
            assert_eq!(
                Dialect::OpenAI.budget(model),
                Budget::MaximumCompletionTokens,
                "{model}"
            );
        }
    }

    #[test]
    fn openai_takes_at_most_four_stops_and_none_of_the_template_tokens() {
        let mut stops = strings(TEMPLATE_STOPS);
        stops.extend(strings(&["User:", "", "Human:", "###", "END", "STOP"]));

        let shaped = Dialect::OpenAI.stops("gpt-4o", &stops);

        assert_eq!(shaped, strings(&["User:", "Human:", "###", "END"]));
    }

    #[test]
    fn anthropic_keeps_every_stop_but_whitespace() {
        let stops = strings(&["<|im_end|>", " ", "\n", "User:"]);

        assert_eq!(
            Dialect::Anthropic.stops("claude-sonnet-4-5", &stops),
            strings(&["<|im_end|>", "User:"])
        );
    }

    #[test]
    fn anthropic_temperature_is_held_between_zero_and_one() {
        assert_eq!(Dialect::Anthropic.temperature("claude", 1.4), Some(1.0));
        assert_eq!(Dialect::Anthropic.temperature("claude", -0.5), Some(0.0));
        assert_eq!(Dialect::Anthropic.temperature("claude", 0.3), Some(0.3));
    }

    #[test]
    fn a_compatible_endpoint_is_sent_the_request_unchanged() {
        let stops = strings(TEMPLATE_STOPS);

        assert_eq!(Dialect::Compatible.stops("o3", &stops), stops);
        assert_eq!(Dialect::Compatible.temperature("o3", 1.7), Some(1.7));
        assert_eq!(Dialect::Compatible.budget("o3"), Budget::MaximumTokens);
        assert!(Dialect::Compatible.effort("anything"));
        assert!(Dialect::Compatible.extended());
        assert_eq!(Dialect::default(), Dialect::Compatible);
    }

    #[test]
    fn a_compatible_endpoint_borrows_the_history_untouched() {
        let mut captured = Message::tool_result("call_1", "captured");
        captured.images = vec![SCREENSHOT.to_string()];
        let history = [captured];

        assert!(matches!(
            Dialect::Compatible.messages(&history),
            Cow::Borrowed(borrowed) if borrowed.len() == 1 && borrowed[0].images.len() == 1
        ));
    }

    #[test]
    fn an_image_waits_until_every_result_of_its_call_has_been_sent() {
        let mut requested = Message::assistant_with_tools(vec![call("call_1"), call("call_2")]);
        requested.images = vec![SCREENSHOT.to_string()];
        let mut first = Message::tool_result("call_1", "one");
        first.images = vec![SCREENSHOT.to_string()];
        let history = [
            requested,
            first,
            Message::tool_result("call_2", "two"),
            Message::user("next"),
        ];

        for dialect in [Dialect::OpenAI, Dialect::Anthropic] {
            let shaped = dialect.messages(&history);

            assert_eq!(
                roles(&shaped),
                vec![
                    (Role::Assistant, None, 0),
                    (Role::Tool, Some("one"), 0),
                    (Role::Tool, Some("two"), 0),
                    (Role::User, Some(ASSISTANT_IMAGE), 1),
                    (
                        Role::User,
                        Some("[Image from the tool result for call_1]"),
                        1
                    ),
                    (Role::User, Some("next"), 0),
                ],
                "{dialect:?}"
            );
        }
    }

    #[test]
    fn an_image_from_the_last_tool_result_still_reaches_the_model() {
        let mut captured = Message::tool_result("call_1", "captured");
        captured.images = vec![SCREENSHOT.to_string()];
        captured.tool_call_id = None;
        captured.name = Some("screenshot".to_string());
        let history = [
            Message::assistant_with_tools(vec![call("call_1")]),
            captured,
        ];

        let shaped = Dialect::OpenAI.messages(&history);

        assert_eq!(
            roles(&shaped),
            vec![
                (Role::Assistant, None, 0),
                (Role::Tool, Some("captured"), 0),
                (Role::User, Some(TOOL_IMAGE), 1),
            ]
        );
        assert_eq!(shaped[1].name, None);
    }

    #[test]
    fn replayed_reasoning_and_blank_assistant_text_are_dropped() {
        let mut requested = Message::assistant_with_tools(vec![call("call_1")]);
        requested.content = Some(" ".to_string());
        requested.reasoning_content = Some("thinking".to_string());
        requested.thinking_blocks = vec![serde_json::json!({ "type": "thinking" })];
        let mut answered = Message::assistant("");
        answered.reasoning_content = Some("thinking".to_string());
        let history = [requested, Message::tool_result("call_1", "done"), answered];

        let shaped = Dialect::Anthropic.messages(&history);

        assert_eq!(
            roles(&shaped),
            vec![(Role::Assistant, None, 0), (Role::Tool, Some("done"), 0)]
        );
        assert!(shaped.iter().all(
            |message| message.reasoning_content.is_none() && message.thinking_blocks.is_empty()
        ));
    }

    #[test]
    fn a_provider_api_is_given_object_schemas_and_never_an_empty_tool_list() {
        let untyped = [
            ToolDefinition::function("now", "The time", serde_json::json!({})),
            ToolDefinition::function("nothing", "Nothing", Value::Null),
            ToolDefinition::function(
                "read",
                "Read",
                serde_json::json!({ "properties": { "path": { "type": "string" } } }),
            ),
        ];

        for dialect in [Dialect::OpenAI, Dialect::Anthropic] {
            assert!(dialect.tools(Some(&[])).is_none(), "{dialect:?}");
            let shaped = dialect.tools(Some(&untyped)).expect("tools");
            let schemas: Vec<&Value> = shaped
                .iter()
                .map(|tool| &tool.function.parameters)
                .collect();

            assert_eq!(
                schemas,
                [
                    &serde_json::json!({ "type": "object", "properties": {} }),
                    &serde_json::json!({ "type": "object", "properties": {} }),
                    &serde_json::json!({
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                    }),
                ],
                "{dialect:?}"
            );
            assert!(matches!(
                dialect.tools(Some(&shaped)),
                Some(Cow::Borrowed(_))
            ));
        }
        assert!(matches!(
            Dialect::Compatible.tools(Some(&[])),
            Some(Cow::Borrowed([]))
        ));
    }

    #[test]
    fn effort_is_sent_only_where_the_api_reads_it() {
        assert!(Dialect::OpenAI.effort("o3"));
        assert!(!Dialect::OpenAI.effort("gpt-4o"));
        assert!(!Dialect::Anthropic.effort("claude-sonnet-4-5"));
        assert!(!Dialect::OpenAI.extended());
        assert!(!Dialect::Anthropic.extended());
    }
}
