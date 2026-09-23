use super::name::sanitize_identifier;
use crate::tools::ToolRegistry;

/// What the model is told about any attached MCP tools, whichever server they
/// came from.
const PREFIXED: &str = "You also have tools from MCP servers. Names are prefixed with the server \
                        name (for example docs_search). Use them when they help the task.";

/// System-prompt text the caller supplies for one server's tools, added only
/// when that server attached at least one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guidance {
    pub server: String,
    pub text: String,
}

impl Guidance {
    pub fn new(server: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            text: text.into(),
        }
    }

    /// Whether any of `names` is a tool this server exported, as
    /// [`qualified_tool_name`](super::qualified_tool_name) would name it.
    fn applies(&self, names: &[&str]) -> bool {
        let server = sanitize_identifier(&self.server);
        let prefix = format!("{server}_");
        names
            .iter()
            .any(|name| *name == server || name.starts_with(&prefix))
    }
}

/// Extra system-prompt text for a registry's MCP tools, if it has any.
pub fn guidance(registry: &ToolRegistry, sections: &[Guidance]) -> Option<String> {
    if !registry.has_mcp() {
        return None;
    }
    guidance_for_tools(&registry.names(), sections)
}

/// Extra system-prompt text for a set of tool names: the prefix rule, then
/// every section whose server exported one of them, in the order given.
pub fn guidance_for_tools(names: &[&str], sections: &[Guidance]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let mut text = String::from(PREFIXED);
    for section in sections.iter().filter(|section| section.applies(names)) {
        text.push_str("\n\n");
        text.push_str(&section.text);
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYBOOK: &str = "magents coordinates other coding agents on this machine.";
    const DELEGATION: &str = "Delegate a question when answering it means reading across \
                              several files.";
    const LAUNDERING: &str = "Permission does not travel between sessions.";

    fn magents() -> Vec<Guidance> {
        vec![
            Guidance::new("magents", PLAYBOOK),
            Guidance::new("magents", DELEGATION),
            Guidance::new("magents", LAUNDERING),
        ]
    }

    #[test]
    fn guidance_omitted_without_mcp_tools() {
        assert!(guidance_for_tools(&[], &magents()).is_none());
        assert!(guidance(&ToolRegistry::with_defaults(), &magents()).is_none());
    }

    #[test]
    fn guidance_mentions_prefix_for_any_mcp_tool() {
        let text = guidance_for_tools(&["docs_search"], &magents()).unwrap();
        assert!(text.contains("prefixed with the server"));
        assert!(!text.contains(PLAYBOOK));
    }

    #[test]
    fn guidance_adds_a_registered_server_playbook() {
        let text = guidance_for_tools(&["read_file", "magents_spawn_session"], &magents()).unwrap();
        assert!(text.contains("prefixed with the server"));
        assert!(text.contains(PLAYBOOK));
    }

    #[test]
    fn guidance_adds_every_section_for_the_server_in_the_order_given() {
        let text = guidance_for_tools(&["read_file", "magents_spawn_session"], &magents()).unwrap();
        let playbook = text.find(PLAYBOOK).expect("the playbook");
        let delegation = text.find(DELEGATION).expect("the delegation section");
        let laundering = text.find(LAUNDERING).expect("the laundering section");
        assert!(playbook < delegation && delegation < laundering, "{text}");
    }

    #[test]
    fn guidance_applies_to_any_tool_the_server_exported() {
        let text = guidance_for_tools(&["magents_send_message"], &magents()).unwrap();
        assert!(text.contains(LAUNDERING));
    }

    /// When a turn started and how long it has run are said once, at the
    /// prompt boundary, so nothing this module adds may say them again.
    #[test]
    fn guidance_leaves_elapsed_time_to_the_prompt_boundary() {
        let text = guidance_for_tools(&["magents_spawn_session"], &[]).unwrap();
        assert!(!text.to_lowercase().contains("elapsed time"));
    }

    #[test]
    fn guidance_omits_a_server_section_when_that_server_attached_nothing() {
        let text = guidance_for_tools(&["docs_search", "read_file"], &magents()).unwrap();
        assert!(!text.contains(PLAYBOOK));
        assert!(!text.contains(DELEGATION));
        assert!(!text.contains(LAUNDERING));
    }

    /// A server whose name had to be sanitized is matched the way its tools
    /// were named, not the way the caller spelled it.
    #[test]
    fn guidance_matches_a_sanitized_server_name() {
        let sections = [Guidance::new("my.docs", PLAYBOOK)];
        let text = guidance_for_tools(&["my_docs_search"], &sections).unwrap();
        assert!(text.contains(PLAYBOOK));
    }
}
