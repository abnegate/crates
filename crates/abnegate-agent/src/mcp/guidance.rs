use super::name::server_prefix;
use crate::tools::ToolRegistry;

/// What the model is told about any attached MCP tools, whichever server they
/// came from.
const PREFIXED: &str = "You also have tools from MCP servers. Names are prefixed with the server \
                        name and two underscores (for example docs__search). Use them when they \
                        help the task.";

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
    ///
    /// Matched on the whole `server__` prefix: a server named `run` claims
    /// `run__deploy`, never the built-in `run_command`.
    fn applies(&self, names: &[&str]) -> bool {
        let prefix = server_prefix(&self.server);
        names.iter().any(|name| name.starts_with(&prefix))
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

    const PLAYBOOK: &str = "notes keeps what the user wrote down on this machine.";
    const DELEGATION: &str = "Search notes before asking the user to repeat themselves.";
    const PRIVACY: &str = "Never quote a note back to anyone but its author.";

    fn notes() -> Vec<Guidance> {
        vec![
            Guidance::new("notes", PLAYBOOK),
            Guidance::new("notes", DELEGATION),
            Guidance::new("notes", PRIVACY),
        ]
    }

    #[test]
    fn guidance_omitted_without_mcp_tools() {
        assert!(guidance_for_tools(&[], &notes()).is_none());
        assert!(guidance(&ToolRegistry::with_defaults(), &notes()).is_none());
    }

    #[test]
    fn guidance_mentions_prefix_for_any_mcp_tool() {
        let text = guidance_for_tools(&["docs__search"], &notes()).unwrap();
        assert!(text.contains("prefixed with the server"));
        assert!(!text.contains(PLAYBOOK));
    }

    #[test]
    fn guidance_adds_a_registered_server_playbook() {
        let text = guidance_for_tools(&["read_file", "notes__search"], &notes()).unwrap();
        assert!(text.contains("prefixed with the server"));
        assert!(text.contains(PLAYBOOK));
    }

    #[test]
    fn guidance_adds_every_section_for_the_server_in_the_order_given() {
        let text = guidance_for_tools(&["read_file", "notes__search"], &notes()).unwrap();
        let playbook = text.find(PLAYBOOK).expect("the playbook");
        let delegation = text.find(DELEGATION).expect("the delegation section");
        let privacy = text.find(PRIVACY).expect("the privacy section");
        assert!(playbook < delegation && delegation < privacy, "{text}");
    }

    #[test]
    fn guidance_applies_to_any_tool_the_server_exported() {
        let text = guidance_for_tools(&["notes__append"], &notes()).unwrap();
        assert!(text.contains(PRIVACY));
    }

    /// When a turn started and how long it has run are said once, at the
    /// prompt boundary, so nothing this module adds may say them again.
    #[test]
    fn guidance_leaves_elapsed_time_to_the_prompt_boundary() {
        let text = guidance_for_tools(&["notes__search"], &[]).unwrap();
        assert!(!text.to_lowercase().contains("elapsed time"));
    }

    #[test]
    fn guidance_omits_a_server_section_when_that_server_attached_nothing() {
        let text = guidance_for_tools(&["docs__search", "read_file"], &notes()).unwrap();
        assert!(!text.contains(PLAYBOOK));
        assert!(!text.contains(DELEGATION));
        assert!(!text.contains(PRIVACY));
    }

    /// A server whose name had to be sanitized is matched the way its tools
    /// were named, not the way the caller spelled it.
    #[test]
    fn guidance_matches_a_sanitized_server_name() {
        let sections = [Guidance::new("my.docs", PLAYBOOK)];
        let text = guidance_for_tools(&["my_docs__search"], &sections).unwrap();
        assert!(text.contains(PLAYBOOK));
    }

    /// Matching on `run_` let a server named `run` claim the built-in
    /// `run_command`, and put its playbook in front of a model that had
    /// none of its tools.
    #[test]
    fn a_server_does_not_claim_a_tool_that_merely_starts_with_its_name() {
        let sections = [Guidance::new("run", PLAYBOOK)];
        let text = guidance_for_tools(&["run_command", "run_shell"], &sections).unwrap();
        assert!(!text.contains(PLAYBOOK), "{text}");
        let text = guidance_for_tools(&["run__deploy"], &sections).unwrap();
        assert!(text.contains(PLAYBOOK), "{text}");
    }
}
