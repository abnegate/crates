/// Per-request output reservation. Must match the context budget calculation.
#[derive(Debug, Clone, Copy)]
pub struct RequestOptions {
    pub reserved: u32,
}
