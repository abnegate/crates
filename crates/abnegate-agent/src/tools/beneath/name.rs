use std::ffi::OsString;

pub(super) enum Name {
    Parent,
    Entry(OsString),
}
