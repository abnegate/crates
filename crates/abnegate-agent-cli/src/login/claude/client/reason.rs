use serde::Deserialize;
use serde::de::IgnoredAny;

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Reason {
    Code(String),
    Detail(IgnoredAny),
}
