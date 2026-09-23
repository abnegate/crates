/// One step from a TOML value into one of its children.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Segment {
    Key(String),
    Index(usize),
}
