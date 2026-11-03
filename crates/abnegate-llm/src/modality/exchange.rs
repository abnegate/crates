use serde::Deserialize;
use serde::Serialize;

const TOKENS_PER_MILLION: f64 = 1_000_000.0;

/// One exchange with a model, as it happened.
///
/// Serialises to one JSON object, so a host can keep a run's exchanges as
/// NDJSON and read them back to replay or score them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Exchange {
    /// The provider that answered.
    pub provider: String,
    /// The model that answered.
    pub model: String,
    /// The schema asked for, when one was.
    pub schema: Option<String>,
    /// The JSON schema the answer was asked to fit, so the same question can
    /// be asked again of another model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<serde_json::Value>,
    /// The system prompt sent.
    pub system_prompt: String,
    /// The user prompt sent.
    pub user_prompt: String,
    /// The answer that was kept.
    pub answer: String,
    /// Every token sent across the attempts this exchange took.
    pub input_tokens: u32,
    /// Every token received across the attempts this exchange took.
    pub output_tokens: u32,
    /// How many requests it took to get an answer that was kept.
    pub attempts: u32,
}

impl Exchange {
    /// What this exchange cost, at the given rate per million tokens.
    ///
    /// A single rate cannot express the input/output split every model has, so
    /// this is indicative. The token counts beside it are exact when the
    /// provider reports them.
    pub fn cost_usd(&self, per_million_tokens: f64) -> f64 {
        let tokens = u64::from(self.input_tokens) + u64::from(self.output_tokens);
        tokens as f64 / TOKENS_PER_MILLION * per_million_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::Exchange;

    const FIELDS: &[&str] = &[
        "provider",
        "model",
        "schema",
        "response_schema",
        "system_prompt",
        "user_prompt",
        "answer",
        "input_tokens",
        "output_tokens",
        "attempts",
    ];

    const GOLDEN: &str = concat!(
        r#"{"provider":"anthropic","model":"claude-opus-5","schema":"the facts","response_schema":{"properties":{"title":{"type":"string"}},"type":"object"},"system_prompt":"You extract facts.","user_prompt":"Requiem, an RPG.","answer":"{\"title\":\"Requiem\"}","input_tokens":12,"output_tokens":7,"attempts":2}"#,
        "\n",
        r#"{"provider":"ollama","model":"llama3","schema":null,"system_prompt":"You are a narrator.","user_prompt":"Write a line.","answer":"Once upon a time.","input_tokens":5,"output_tokens":4,"attempts":1}"#,
        "\n",
    );

    fn exchange(input_tokens: u32, output_tokens: u32) -> Exchange {
        Exchange {
            provider: "anthropic".into(),
            model: "claude-opus-5".into(),
            schema: None,
            response_schema: None,
            system_prompt: String::new(),
            user_prompt: String::new(),
            answer: String::new(),
            input_tokens,
            output_tokens,
            attempts: 1,
        }
    }

    #[test]
    fn a_cost_is_derived_from_the_tokens_that_were_used() {
        assert!((exchange(500_000, 500_000).cost_usd(25.0) - 25.0).abs() < 1e-9);
    }

    #[test]
    fn a_cost_never_overflows_the_counters() {
        let cost = exchange(u32::MAX, u32::MAX).cost_usd(1.0);
        assert!((cost - 2.0 * f64::from(u32::MAX) / 1_000_000.0).abs() < 1e-6);
    }

    fn recorded() -> Vec<Exchange> {
        vec![
            Exchange {
                provider: "anthropic".into(),
                model: "claude-opus-5".into(),
                schema: Some("the facts".into()),
                response_schema: Some(serde_json::json!({
                    "properties": { "title": { "type": "string" } },
                    "type": "object",
                })),
                system_prompt: "You extract facts.".into(),
                user_prompt: "Requiem, an RPG.".into(),
                answer: r#"{"title":"Requiem"}"#.into(),
                input_tokens: 12,
                output_tokens: 7,
                attempts: 2,
            },
            Exchange {
                provider: "ollama".into(),
                model: "llama3".into(),
                schema: None,
                response_schema: None,
                system_prompt: "You are a narrator.".into(),
                user_prompt: "Write a line.".into(),
                answer: "Once upon a time.".into(),
                input_tokens: 5,
                output_tokens: 4,
                attempts: 1,
            },
        ]
    }

    fn ndjson(exchanges: &[Exchange]) -> String {
        exchanges
            .iter()
            .map(|exchange| serde_json::to_string(exchange).unwrap() + "\n")
            .collect()
    }

    #[test]
    fn exchanges_are_written_as_the_golden_ndjson() {
        assert_eq!(ndjson(&recorded()), GOLDEN);
    }

    #[test]
    fn the_golden_ndjson_reads_back_as_the_exchanges_that_wrote_it() {
        let read: Vec<Exchange> = GOLDEN
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        assert_eq!(read, recorded());
    }

    #[test]
    fn the_wire_names_of_every_field_are_pinned() {
        let value = serde_json::to_value(&recorded()[0]).unwrap();
        let mut names: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut expected = FIELDS.to_vec();
        names.sort_unstable();
        expected.sort_unstable();

        assert_eq!(names, expected);
    }

    #[test]
    fn an_exchange_without_a_response_schema_leaves_it_out() {
        let value = serde_json::to_value(&recorded()[1]).unwrap();

        assert!(value.get("response_schema").is_none(), "{value}");
    }
}
