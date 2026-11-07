pub mod outer {
    pub mod middle {
        pub mod inner {
            pub fn deepest(value: u32) -> u32 {
                value.wrapping_mul(31).wrapping_add(7)
            }

            pub struct InnerState {
                pub counter: u64,
            }
        }

        pub fn middle_helper(values: &[u32]) -> u32 {
            values.iter().map(|value| inner::deepest(*value)).sum()
        }
    }

    pub fn outer_entry() -> u32 {
        middle::middle_helper(&[1, 2, 3])
    }
}
