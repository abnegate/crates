use std::collections::HashMap;
use std::collections::HashSet;

use abnegate_llm::Role;
use sha2::Digest;
use sha2::Sha256;

use super::Entry;
use super::Summary;

/// A conversation as loaded from the store.
#[derive(Debug, Clone, Default)]
pub struct History {
    pub entries: Vec<Entry>,
    pub summary: Option<Summary>,
    pub latest_user: Option<String>,
    pub incomplete: bool,
}

/// Same v1 tuple encoding as [`coverage`](crate::context::coverage), excluding mutable flags.
pub fn fingerprint(entries: &[Entry], ids: &[String]) -> Result<String, String> {
    let mut selected = Vec::with_capacity(ids.len());
    let mut seen = HashSet::new();
    let mut cursor = 0;
    for entry in entries {
        if !seen.insert(entry.id.as_str()) {
            return Err("Duplicate canonical entry identity".into());
        }
        if ids.get(cursor) == Some(&entry.id) {
            let message = &entry.message;
            selected.push((
                &entry.id,
                message.role,
                &message.content,
                &message.name,
                &message.tool_calls,
                &message.tool_call_id,
                &message.images,
                &message.generated_images,
            ));
            cursor += 1;
        }
    }
    if cursor != ids.len() {
        return Err("Coverage entries are missing, duplicated, or out of order".into());
    }
    let bytes = serde_json::to_vec(&selected).map_err(|error| error.to_string())?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Verify storage integrity before a checkpoint may suppress any canonical evidence.
pub fn validate(history: &History, summary: &Summary) -> Result<(), String> {
    if summary.content.trim().is_empty() || summary.entries.is_empty() || summary.revision == 0 {
        return Err("Empty or invalid conversation checkpoint".into());
    }
    if fingerprint(&history.entries, &summary.entries)? != summary.fingerprint {
        return Err("Conversation checkpoint evidence fingerprint does not match".into());
    }
    let selected: HashSet<&str> = summary.entries.iter().map(String::as_str).collect();
    let mut calls = HashMap::new();
    let mut results = HashMap::new();
    for entry in &history.entries {
        let included = selected.contains(entry.id.as_str());
        if included
            && (entry.message.role == Role::System
                || history.latest_user.as_ref() == Some(&entry.id))
        {
            return Err("Checkpoint covers protected instructions or current user request".into());
        }
        if let Some(envelope) = &entry.message.tool_calls {
            if entry.message.role != Role::Assistant || envelope.is_empty() {
                return Err("Invalid canonical tool envelope".into());
            }
            for call in envelope {
                if calls
                    .insert(call.id.as_str(), (included, entry.consumed))
                    .is_some()
                {
                    return Err("Duplicate canonical tool call".into());
                }
            }
        }
        if entry.message.role == Role::Tool {
            let id = entry
                .message
                .tool_call_id
                .as_deref()
                .ok_or("Tool result has no call identity")?;
            if results.insert(id, (included, entry.consumed)).is_some() {
                return Err("Duplicate canonical tool result".into());
            }
        }
    }
    for (id, (included, consumed)) in calls {
        match results.get(id) {
            Some((result_included, result_consumed)) if included == *result_included => {
                if included && (!consumed || !result_consumed) {
                    return Err("Checkpoint covers unconsumed tool evidence".into());
                }
            }
            None if !included => (),
            _ => return Err("Checkpoint splits a tool call/result group".into()),
        }
        results.remove(id);
    }
    if results.values().any(|(included, _)| *included) {
        return Err("Checkpoint contains an orphan tool result".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use abnegate_llm::GeneratedImage;
    use abnegate_llm::Message;

    use super::*;
    use crate::chat::ReplayMessage;

    #[test]
    fn storage_roundtrip_preserves_multimodal_messages() {
        let mut message = Message::user("Inspect this exact image");
        message.images.push("/api/artifacts/source.png".into());
        message
            .generated_images
            .push(GeneratedImage::new("data:image/png;base64,AA=="));
        let replay = ReplayMessage::from(&message);
        let restored: ReplayMessage =
            serde_json::from_slice(&serde_json::to_vec(&replay).unwrap()).unwrap();
        let restored = restored.into_message();
        assert_eq!(restored.images, message.images);
        assert_eq!(restored.content, message.content);
        assert_eq!(
            restored.generated_images[0].image_url.url,
            message.generated_images[0].image_url.url
        );
    }

    #[test]
    fn evidence_digest_includes_images_and_exact_error_suffix() {
        let mut entry = Entry {
            id: "entry".into(),
            message: ReplayMessage::from(&Message::tool_result(
                "call",
                "Error: failed\nlong evidence suffix",
            )),
            consumed: true,
        };
        let ids = vec![entry.id.clone()];
        let original = fingerprint(&[entry.clone()], &ids).unwrap();
        let canonical = crate::context::Entry {
            id: entry.id.clone(),
            message: entry.message.clone().into_message(),
            preserve: false,
            consumed: entry.consumed,
        };
        assert_eq!(
            original,
            crate::context::coverage(&[canonical], &ids)
                .unwrap()
                .fingerprint
        );
        entry
            .message
            .images
            .push("https://example.com/evidence.png".into());
        assert_ne!(original, fingerprint(&[entry.clone()], &ids).unwrap());
        entry.message.images.clear();
        entry.message.content = Some("Error: failed".into());
        assert_ne!(original, fingerprint(&[entry], &ids).unwrap());
    }
}
