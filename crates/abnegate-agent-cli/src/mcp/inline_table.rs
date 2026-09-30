use std::fmt;
use std::fmt::Write;

/// A TOML inline table, as Codex parses the value of a `-c key=value`
/// override.
///
/// Every key and every string is written as a TOML basic string, so no text
/// a configuration holds can end one early or add an entry of its own.
#[derive(Debug, Default)]
pub(crate) struct InlineTable {
    entries: Vec<String>,
}

impl InlineTable {
    /// The same table, with `key` set to the string `value`.
    pub(crate) fn with_text(self, key: &str, value: &str) -> Self {
        self.with(key, quoted(value))
    }

    /// The same table, with `key` set to the array of strings `values`.
    pub(crate) fn with_texts<I>(self, key: &str, values: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let items: Vec<String> = values
            .into_iter()
            .map(|value| quoted(value.as_ref()))
            .collect();
        self.with(key, format!("[{}]", items.join(", ")))
    }

    /// The same table, with `key` set to `table`.
    pub(crate) fn with_table(self, key: &str, table: InlineTable) -> Self {
        self.with(key, table.to_string())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn with(mut self, key: &str, value: String) -> Self {
        self.entries.push(format!("{} = {value}", quoted(key)));
        self
    }
}

impl fmt::Display for InlineTable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.entries.is_empty() {
            return formatter.write_str("{}");
        }
        write!(formatter, "{{ {} }}", self.entries.join(", "))
    }
}

/// `text` as a TOML basic string: quoted, with `"`, `\` and every control
/// character escaped.
fn quoted(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(quoted, "\\u{:04X}", u32::from(control));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::InlineTable;

    #[test]
    fn an_empty_table_is_written_as_one() {
        assert_eq!(InlineTable::default().to_string(), "{}");
        assert!(InlineTable::default().is_empty());
    }

    #[test]
    fn keys_and_strings_are_quoted_and_nested_tables_inlined() {
        let table = InlineTable::default()
            .with_text("command", "uvx")
            .with_texts("args", ["mcp-server", "--flag"])
            .with_table(
                "env_http_headers",
                InlineTable::default().with_text("Authorization", "ABNEGATE_MCP_T_0"),
            );

        assert_eq!(
            table.to_string(),
            r#"{ "command" = "uvx", "args" = ["mcp-server", "--flag"], "env_http_headers" = { "Authorization" = "ABNEGATE_MCP_T_0" } }"#
        );
    }

    /// A value that could close its string early would let a configuration
    /// add a key of its own, such as a second command.
    #[test]
    fn no_text_can_end_its_string_early() {
        let table = InlineTable::default().with_text(
            "a\" = 1, \"b",
            "x\", \"command\" = \"sh\\\u{1b}\u{7f}\n\t\r",
        );

        assert_eq!(
            table.to_string(),
            r#"{ "a\" = 1, \"b" = "x\", \"command\" = \"sh\\\u001B\u007F\n\t\r" }"#
        );
    }
}
