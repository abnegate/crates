/// Provider `reasoning_effort` once Auto and Off have been resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Effort;

    #[test]
    fn a_resolved_effort_renders_the_level_the_provider_expects() {
        assert_eq!(Effort::Low.as_str(), "low");
        assert_eq!(Effort::Medium.as_str(), "medium");
        assert_eq!(Effort::High.as_str(), "high");
    }
}
