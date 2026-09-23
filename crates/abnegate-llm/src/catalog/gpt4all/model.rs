use serde::Deserialize;
use serde::Deserializer;
use serde::de;

#[derive(Debug, Deserialize)]
pub(crate) struct Gpt4AllModel {
    pub(crate) name: String,
    pub(crate) filename: String,
    #[serde(deserialize_with = "string_or_number")]
    pub(crate) filesize: u64,
    #[serde(default)]
    pub(crate) parameters: Option<String>,
    #[serde(rename = "type", default)]
    pub(crate) model_type: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) quant: Option<String>,
    #[serde(rename = "ramrequired", default)]
    pub(crate) ram_required: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) url: Option<String>,
}

impl Gpt4AllModel {
    pub(crate) fn matches(&self, needle: &str) -> bool {
        self.name.to_lowercase().contains(needle)
            || self.filename.to_lowercase().contains(needle)
            || self
                .description
                .as_ref()
                .is_some_and(|value| value.to_lowercase().contains(needle))
            || self
                .model_type
                .as_ref()
                .is_some_and(|value| value.to_lowercase().contains(needle))
    }
}

/// GPT4All writes some sizes as numbers and some as strings of digits.
fn string_or_number<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| de::Error::custom(format!("{number} is not a byte count"))),
        serde_json::Value::String(text) => text.parse().map_err(de::Error::custom),
        other => Err(de::Error::custom(format!("{other} is not a byte count"))),
    }
}
