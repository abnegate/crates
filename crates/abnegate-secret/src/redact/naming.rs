/// Which values a key word names are taken for credentials.
#[derive(Clone, Copy)]
pub(super) enum Naming {
    /// Every one, whatever it looks like, as text on its way to a log needs:
    /// a chosen passphrase reads as words.
    Any,
    /// Only one shaped like a credential, as a value meant to be a plain name
    /// needs: in a name, `token:refresh` is a script, not an assignment.
    Shaped,
}
