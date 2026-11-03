use serde::Deserialize;
use serde_json::Value;

/// Where ComfyUI lists a queue entry's prompt id: each entry is `[number,
/// prompt_id, prompt, extra_data, outputs_to_execute]`.
const PROMPT_ID: usize = 1;

/// The prompts ComfyUI is running and has yet to run, as `GET /queue` lists
/// them.
#[derive(Debug, Deserialize)]
pub(super) struct Queue {
    queue_running: Vec<Value>,
    queue_pending: Vec<Value>,
}

impl Queue {
    /// Whether `prompt_id` is running or waiting to run.
    pub(super) fn holds(&self, prompt_id: &str) -> bool {
        self.queue_running
            .iter()
            .chain(&self.queue_pending)
            .any(|entry| entry.get(PROMPT_ID).and_then(Value::as_str) == Some(prompt_id))
    }
}
