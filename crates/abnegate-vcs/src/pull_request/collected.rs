/// Every row a paged read reached, in the order GitHub listed them, and
/// whether that is every row GitHub holds.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Collected<T> {
    /// The rows read, in the order GitHub listed them.
    pub items: Vec<T>,
    /// Whether `items` is every row there is. It is not when GitHub counted
    /// more rows than the read reached; when the read stopped at its page
    /// limit on a full page, unless GitHub counted exactly as many rows as it
    /// read; or when the rows moved between pages while it read, which shows
    /// as a row GitHub listed twice, or, past one page, as rows that cannot
    /// be told apart. A row it did not reach may then say something the rows
    /// it did reach do not.
    ///
    /// GitHub lists rows a page at a time, never as they all stood at one
    /// moment. A row removed from a page already read, while another is added
    /// to a page not yet read, leaves the count and every row as they would
    /// be had nothing moved, so no read can rule that out.
    pub complete: bool,
}

impl<T> Collected<T> {
    /// `items`, which are every row there is when `complete` says so.
    pub fn new(items: Vec<T>, complete: bool) -> Self {
        Self { items, complete }
    }
}
