/// What a text cut short at a byte limit ends with, on a line of its own, so
/// a reader knows it holds only the start of what there was.
pub const MARKER: &str = "\n...[truncated]";
