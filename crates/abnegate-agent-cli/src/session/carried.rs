//! A session's file once it is written into another home.

use std::path::PathBuf;

/// The file [`Session::carry`](crate::Session::carry) wrote a session into,
/// in the home it was carried to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Carried {
    /// Where the copy is, under the receiving home.
    pub path: PathBuf,
    /// How many bytes were copied.
    pub bytes: u64,
}

impl Carried {
    /// A copy of `bytes` bytes at `path`.
    pub fn new(path: impl Into<PathBuf>, bytes: u64) -> Self {
        Self {
            path: path.into(),
            bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::Carried;

    #[test]
    fn a_carried_copy_keeps_its_path_and_size() {
        let carried = Carried::new("/home/agent/projects/-work/id.jsonl", 42);

        assert_eq!(
            carried.path,
            Path::new("/home/agent/projects/-work/id.jsonl")
        );
        assert_eq!(carried.bytes, 42);
    }
}
