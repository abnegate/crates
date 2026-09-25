use std::collections::BTreeMap;
use std::path::PathBuf;

use abnegate_secret::SecretValue;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;

use crate::mcp::mismatch::Mismatch;
use crate::mcp::server::McpServer;
use crate::mcp::transport::McpTransport;

const COMMAND: &[&str] = &["command"];
const ARGUMENTS: &[&str] = &["args", "arguments"];
const ENVIRONMENT: &[&str] = &["env", "environment"];
const URL: &[&str] = &["url"];
const TRANSPORT: &[&str] = &["type"];
const HEADERS: &[&str] = &["headers"];
const TOOLS: &[&str] = &["tools"];
const WORKING_DIRECTORY: &[&str] = &["cwd", "working_directory"];
const INHERIT_ENVIRONMENT: &[&str] = &["inherit_environment"];
const DISABLED: &[&str] = &["disabled"];

const TOOL_NAME: &str = "name";

const OBJECT: &str = "an object";
const TEXT: &str = "a string";
const TEXTS: &str = "a list of strings";
const TEXT_MAP: &str = "an object of strings";
const TOOL_LIST: &str = "a list of tool names, or of objects each with a `name`";
const FLAG: &str = "true or false";
const ONCE: &str = "given under one name";

/// One server's entry in an MCP configuration document, read field by field
/// so that a value of the wrong JSON type is reported by the field holding
/// it, never quoted. A field this crate does not read, such as Claude Code's
/// `timeout`, is ignored.
pub(crate) struct Entry<'document> {
    fields: &'document Map<String, Value>,
}

impl<'document> Entry<'document> {
    pub(crate) fn new(entry: &'document Value) -> Result<Self, Mismatch> {
        entry
            .as_object()
            .map(|fields| Self { fields })
            .ok_or(Mismatch::new(None, OBJECT))
    }

    /// The server this entry describes.
    pub(crate) fn server(&self) -> Result<McpServer, Mismatch> {
        Ok(McpServer {
            command: self.text(COMMAND)?,
            arguments: self.texts(ARGUMENTS)?,
            environment: self.secrets(ENVIRONMENT)?,
            url: self.text(URL)?,
            transport: self.transport()?,
            headers: self.secrets(HEADERS)?,
            tools: self.tools()?,
            working_directory: self.text(WORKING_DIRECTORY)?.map(PathBuf::from),
            inherit_environment: self.flag(INHERIT_ENVIRONMENT)?,
            disabled: self.flag(DISABLED)?,
        })
    }

    /// The value under whichever of `names` the entry holds: the field's
    /// name on the wire, first, or another it is also read under.
    fn value(&self, names: &[&'static str]) -> Result<Option<&'document Value>, Mismatch> {
        let mut present = names.iter().filter_map(|name| self.fields.get(*name));
        let value = present.next();
        if present.next().is_some() {
            return Err(Mismatch::new(Some(names[0]), ONCE));
        }
        Ok(value)
    }

    fn text(&self, names: &[&'static str]) -> Result<Option<String>, Mismatch> {
        match self.value(names)? {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(Mismatch::new(Some(names[0]), TEXT)),
        }
    }

    fn texts(&self, names: &[&'static str]) -> Result<Vec<String>, Mismatch> {
        let Some(value) = self.value(names)? else {
            return Ok(Vec::new());
        };
        value
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .ok_or(Mismatch::new(Some(names[0]), TEXTS))
    }

    fn secrets(&self, names: &[&'static str]) -> Result<BTreeMap<String, SecretValue>, Mismatch> {
        let Some(value) = self.value(names)? else {
            return Ok(BTreeMap::new());
        };
        value
            .as_object()
            .and_then(|values| {
                values
                    .iter()
                    .map(|(key, value)| {
                        value
                            .as_str()
                            .map(|text| (key.clone(), SecretValue::new(text)))
                    })
                    .collect()
            })
            .ok_or(Mismatch::new(Some(names[0]), TEXT_MAP))
    }

    fn flag(&self, names: &[&'static str]) -> Result<bool, Mismatch> {
        match self.value(names)? {
            None => Ok(false),
            Some(Value::Bool(flag)) => Ok(*flag),
            Some(_) => Err(Mismatch::new(Some(names[0]), FLAG)),
        }
    }

    /// The transport named, any name this crate does not attach over
    /// reading as [`McpTransport::Unsupported`].
    fn transport(&self) -> Result<Option<McpTransport>, Mismatch> {
        match self.value(TRANSPORT)? {
            None | Some(Value::Null) => Ok(None),
            Some(value) => McpTransport::deserialize(value)
                .map(Some)
                .map_err(|_| Mismatch::new(Some(TRANSPORT[0]), TEXT)),
        }
    }

    /// The tools named, each by a name or by an object with a `name`, the
    /// shape some clients list a server's tools in.
    fn tools(&self) -> Result<Vec<String>, Mismatch> {
        let Some(value) = self.value(TOOLS)? else {
            return Ok(Vec::new());
        };
        value
            .as_array()
            .and_then(|tools| {
                tools
                    .iter()
                    .map(|tool| {
                        tool.as_str()
                            .or_else(|| tool.get(TOOL_NAME).and_then(Value::as_str))
                            .map(str::to_string)
                    })
                    .collect()
            })
            .ok_or(Mismatch::new(Some(TOOLS[0]), TOOL_LIST))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use abnegate_secret::SecretValue;
    use serde_json::json;

    use super::Entry;
    use crate::mcp::mismatch::Mismatch;
    use crate::mcp::server::McpServer;
    use crate::mcp::transport::McpTransport;

    fn read(entry: serde_json::Value) -> Result<McpServer, Mismatch> {
        Entry::new(&entry).and_then(|entry| entry.server())
    }

    #[test]
    fn every_field_reads_under_its_wire_name() {
        let server = read(json!({
            "command": "notes-server",
            "args": ["mcp"],
            "env": {"NOTES_TOKEN": "token"},
            "url": null,
            "type": "stdio",
            "headers": {},
            "tools": ["search"],
            "cwd": "/srv/notes",
            "inherit_environment": true,
            "disabled": false,
            "timeout": 30000
        }))
        .expect("a server");

        assert_eq!(server.command.as_deref(), Some("notes-server"));
        assert_eq!(server.arguments, ["mcp"]);
        assert_eq!(
            server
                .environment
                .get("NOTES_TOKEN")
                .map(SecretValue::expose),
            Some("token")
        );
        assert!(server.url.is_none());
        assert_eq!(server.transport, Some(McpTransport::Stdio));
        assert_eq!(server.tools, ["search"]);
        assert_eq!(server.working_directory, Some(PathBuf::from("/srv/notes")));
        assert!(server.inherit_environment);
        assert!(!server.disabled);
    }

    #[test]
    fn a_value_of_the_wrong_type_names_its_field_and_never_its_value() {
        for (entry, field, expected) in [
            (json!({"command": 1}), "command", "a string"),
            (json!({"args": "mcp"}), "args", "a list of strings"),
            (json!({"arguments": [1]}), "args", "a list of strings"),
            (json!({"env": ["secret"]}), "env", "an object of strings"),
            (
                json!({"headers": "Bearer secret"}),
                "headers",
                "an object of strings",
            ),
            (
                json!({"headers": {"X": 1}}),
                "headers",
                "an object of strings",
            ),
            (json!({"type": 5}), "type", "a string"),
            (
                json!({"tools": [{"description": "search"}]}),
                "tools",
                "a list of tool names, or of objects each with a `name`",
            ),
            (
                json!({"tools": "search"}),
                "tools",
                "a list of tool names, or of objects each with a `name`",
            ),
            (json!({"cwd": false}), "cwd", "a string"),
            (json!({"disabled": "yes"}), "disabled", "true or false"),
            (
                json!({"args": [], "arguments": []}),
                "args",
                "given under one name",
            ),
        ] {
            assert_eq!(
                read(entry.clone()).expect_err("a mismatch"),
                Mismatch::new(Some(field), expected),
                "{entry}"
            );
        }
        assert_eq!(
            read(json!("notes-server")).expect_err("a mismatch"),
            Mismatch::new(None, "an object")
        );
    }

    #[test]
    fn tools_read_by_name_whether_named_or_described() {
        let server = read(json!({
            "url": "https://docs.example.com/mcp",
            "tools": ["fetch", {"name": "search", "description": "Search the docs"}]
        }))
        .expect("a server");

        assert_eq!(server.tools, ["fetch", "search"]);
    }

    #[test]
    fn a_transport_this_crate_does_not_attach_over_is_read_as_unsupported() {
        let server = read(json!({"type": "ws", "url": "wss://mcp.example.com"})).expect("a server");

        assert_eq!(server.transport, Some(McpTransport::Unsupported));
        assert!(!server.valid());
    }
}
