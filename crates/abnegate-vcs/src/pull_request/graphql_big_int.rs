use serde::Deserialize;
use serde::Deserializer;
use serde::de::Error;
use serde_json::Value;

/// A whole number GitHub's GraphQL API sends as its `BigInt` scalar: a
/// string of digits, or a number from an install that sends one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GraphQlBigInt(pub(super) u64);

impl<'de> Deserialize<'de> for GraphQlBigInt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let read = match Value::deserialize(deserializer)? {
            Value::String(digits) => digits.parse().ok(),
            Value::Number(number) => number.as_u64(),
            _ => None,
        };
        read.map(Self)
            .ok_or_else(|| D::Error::custom("not a whole number GitHub's BigInt holds"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_big_int_reads_from_digits_or_a_number() {
        for (sent, expected) in [
            (json!("6814185533"), Some(6_814_185_533)),
            (json!(42), Some(42)),
            (json!("-1"), None),
            (json!("12a"), None),
            (json!(-1), None),
            (json!(1.5), None),
            (json!(null), None),
            (json!(true), None),
        ] {
            let read = serde_json::from_value::<GraphQlBigInt>(sent.clone()).ok();

            assert_eq!(read.map(|number| number.0), expected, "{sent}");
        }
    }
}
