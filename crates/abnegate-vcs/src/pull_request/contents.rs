/// A file's contents as GitHub's contents API describes it at a ref.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Contents {
    /// The file's bytes, decoded from the base64 GitHub sent them in, or
    /// `None` where GitHub's answer carried no content at all, as it does for
    /// an entry that is not a regular file. GitHub sends a file it will not
    /// inline, one past a megabyte, as empty content, so an empty file and
    /// one GitHub left out both read as `Some` of no bytes, and only
    /// [`Self::size`] tells them apart.
    pub content: Option<Vec<u8>>,
    /// The file's size in bytes as GitHub reports it, whether or not it
    /// inlined the file's bytes, or `None` where its answer left the size
    /// out. Empty content with a size above zero is a file GitHub declined
    /// to inline.
    pub size: Option<u64>,
}

impl Contents {
    /// A file holding `content`, or none where GitHub sent none, of `size`
    /// bytes where GitHub reported a size.
    pub fn new(content: Option<Vec<u8>>, size: Option<u64>) -> Self {
        Self { content, size }
    }
}
