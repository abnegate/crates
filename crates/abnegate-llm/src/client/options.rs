/// Per-request output reservation. Must match the context budget calculation.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RequestOptions {
    pub reserved: u32,
}

impl RequestOptions {
    /// Reserve `reserved` tokens of the context for the answer.
    pub fn new(reserved: u32) -> Self {
        Self { reserved }
    }
}
