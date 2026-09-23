use std::collections::HashMap;

pub struct Fill<'a> {
    pub prompt: &'a str,
    pub seed: u64,
    pub weights: HashMap<&'a str, &'a str>,
    pub source: Option<&'a str>,
}
