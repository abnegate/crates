use std::fs::OpenOptions;

use nix::fcntl::OFlag;

/// What the caller intends to do with the descriptor it asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    /// Create the file, or truncate what is already there.
    Replace,
    /// Create the file, or write past what is already there.
    Append,
}

impl Access {
    pub(super) fn flags(self) -> OFlag {
        match self {
            Self::Read => OFlag::O_RDONLY,
            Self::Replace => OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC,
            Self::Append => OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_APPEND,
        }
    }

    pub(super) fn options(self) -> OpenOptions {
        let mut options = OpenOptions::new();
        match self {
            Self::Read => options.read(true),
            Self::Replace => options.write(true).create(true).truncate(true),
            Self::Append => options.append(true).create(true),
        };
        options
    }
}
