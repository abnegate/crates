use crate::object_id::ObjectId;
use crate::pull_request::ObjectKind;

/// A git reference as GitHub holds it: the object it points at, and what kind
/// of object that is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GitReference {
    /// The object the reference points at: for a tag, its annotated tag
    /// object, or the commit itself when the tag is lightweight.
    pub object: ObjectId,
    /// What kind of object [`Self::object`] is.
    pub kind: ObjectKind,
}

impl GitReference {
    /// A reference pointing at `object`, which is a `kind`.
    pub fn new(object: ObjectId, kind: ObjectKind) -> Self {
        Self { object, kind }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_holds_what_it_was_made_with() {
        let object = ObjectId::parse(&"a".repeat(40)).unwrap();

        let reference = GitReference::new(object.clone(), ObjectKind::Tag);

        assert_eq!(reference.object, object);
        assert_eq!(reference.kind, ObjectKind::Tag);
    }
}
