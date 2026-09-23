use abnegate_llm::Message;

/// An immutable replay message plus request-local eligibility flags.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub message: Message,
    pub preserve: bool,
    pub consumed: bool,
}
