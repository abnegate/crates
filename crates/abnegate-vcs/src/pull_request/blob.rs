use crate::object_id::ObjectId;

/// A git blob's content, whole.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Blob {
    /// The blob's object name.
    pub sha: ObjectId,
    /// Its bytes, exactly as git stores them.
    pub content: Vec<u8>,
}

impl Blob {
    /// The blob `sha` names, holding `content`.
    pub fn new(sha: ObjectId, content: Vec<u8>) -> Self {
        Self { sha, content }
    }
}
