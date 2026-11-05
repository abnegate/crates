/// Every row a paged read reached, in the order GitHub listed them, and
/// whether that is every row GitHub holds.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Collected<T> {
    /// The rows read, in the order GitHub listed them.
    pub items: Vec<T>,
    /// Whether `items` is every row there is. It is not when the read stopped
    /// at its page limit on a full page, or read fewer rows than GitHub
    /// counted, and then a row it did not reach may say something the rows it
    /// did reach do not.
    pub complete: bool,
}

impl<T> Collected<T> {
    /// `items`, which are every row there is when `complete` says so.
    pub fn new(items: Vec<T>, complete: bool) -> Self {
        Self { items, complete }
    }
}
