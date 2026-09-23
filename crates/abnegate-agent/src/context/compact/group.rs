use abnegate_llm::Role;
use std::collections::HashSet;

use crate::context::{ContextError, Entry};

/// A message together with the tool results that answer it, which compaction
/// takes or leaves as one.
pub(super) struct Group {
    pub(super) indices: Vec<usize>,
    pub(super) eligible: bool,
    pub(super) complete: bool,
}

pub(super) fn groups(entries: &[Entry]) -> Result<Vec<Group>, ContextError> {
    let mut ids = HashSet::new();
    let mut calls = HashSet::new();
    for entry in entries {
        if entry.id.is_empty() || !ids.insert(&entry.id) {
            return Err(ContextError::Integrity(
                "Entry ids must be unique and nonempty.".into(),
            ));
        }
    }
    let mut output = Vec::new();
    let mut index = 0;
    while index < entries.len() {
        let entry = &entries[index];
        if entry.message.role == Role::Tool {
            return Err(ContextError::Integrity(format!(
                "Unpaired tool result {}.",
                entry.id
            )));
        }
        let mut indices = vec![index];
        let mut complete = true;
        if let Some(envelope) = entry
            .message
            .tool_calls
            .as_ref()
            .filter(|calls| !calls.is_empty())
        {
            if entry.message.role != Role::Assistant {
                return Err(ContextError::Integrity(
                    "Tool calls require an assistant envelope.".into(),
                ));
            }
            let mut expected = HashSet::new();
            for call in envelope {
                if call.id.is_empty()
                    || !calls.insert(&call.id)
                    || !expected.insert(call.id.as_str())
                {
                    return Err(ContextError::Integrity(
                        "Tool call ids must be unique and nonempty.".into(),
                    ));
                }
            }
            while let Some(result) = entries.get(index + indices.len()) {
                if result.message.role != Role::Tool {
                    break;
                }
                if !result
                    .message
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| expected.remove(id))
                {
                    return Err(ContextError::Integrity(format!(
                        "Duplicate or unmatched result {}.",
                        result.id
                    )));
                }
                indices.push(index + indices.len());
            }
            complete = expected.is_empty();
        }
        let eligible = complete
            && indices.iter().all(|&index| {
                let entry = &entries[index];
                !entry.preserve
                    && entry.consumed
                    && entry.message.role != Role::System
                    && entry.message.images.is_empty()
                    && entry.message.generated_images.is_empty()
            });
        index += indices.len();
        output.push(Group {
            indices,
            eligible,
            complete,
        });
    }
    Ok(output)
}
