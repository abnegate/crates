use super::ReplayMessage;

/// A stored message and whether the model has already been shown it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub message: ReplayMessage,
    pub consumed: bool,
}
