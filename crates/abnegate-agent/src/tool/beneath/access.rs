use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;

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
    /// The open flags for this access, never waiting on the far end of a
    /// FIFO or a device: the open returns at once, and whatever it opened is
    /// checked for being a regular file before it is used.
    pub(super) fn flags(self) -> OFlag {
        let access = match self {
            Self::Read => OFlag::O_RDONLY,
            Self::Replace => OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC,
            Self::Append => OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_APPEND,
        };
        access | OFlag::O_NONBLOCK
    }

    pub(super) fn options(self) -> OpenOptions {
        let mut options = OpenOptions::new();
        match self {
            Self::Read => options.read(true),
            Self::Replace => options.write(true).create(true).truncate(true),
            Self::Append => options.append(true).create(true),
        };
        options.custom_flags(OFlag::O_NONBLOCK.bits());
        options
    }
}
