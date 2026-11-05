use std::num::NonZeroU32;

/// One page of a list GitHub answers a page at a time, and the page after it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Page<T> {
    /// What the page holds, in the order GitHub listed it.
    pub items: Vec<T>,
    /// The page to ask for next, or `None` when GitHub sent fewer rows than
    /// were asked for, so this page was the last.
    pub next: Option<NonZeroU32>,
}

impl<T> Page<T> {
    /// `items`, followed by page `next` when there is one.
    pub fn new(items: Vec<T>, next: Option<NonZeroU32>) -> Self {
        Self { items, next }
    }

    /// `items` read as page `page`, which GitHub answered with `read` rows of
    /// the `asked` asked for, counted before any were left out.
    pub(super) fn after(items: Vec<T>, page: NonZeroU32, read: usize, asked: usize) -> Self {
        let next = (read >= asked).then(|| page.checked_add(1)).flatten();
        Self { items, next }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    #[test]
    fn a_full_page_is_followed_and_a_short_one_is_the_last() {
        assert_eq!(Page::after(vec![1], page(2), 10, 10).next, Some(page(3)));
        assert_eq!(Page::after(vec![1], page(2), 9, 10).next, None);
        assert_eq!(
            Page::<u8>::after(Vec::new(), page(u32::MAX), 10, 10).next,
            None
        );
    }
}
