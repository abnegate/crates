/// A ChatGPT plan a codex sign-in can be on, as the `chatgpt_plan_type`
/// claim of codex's id token spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Plan {
    /// ChatGPT Free, spelled `free`.
    Free,
    /// ChatGPT Plus, spelled `plus`.
    Plus,
    /// ChatGPT Pro, spelled `pro`.
    Pro,
    /// ChatGPT Team, spelled `team`.
    Team,
    /// ChatGPT Business, spelled `business`.
    Business,
    /// ChatGPT Enterprise, spelled `enterprise`.
    Enterprise,
    /// ChatGPT Edu, spelled `edu`.
    Edu,
}

impl Plan {
    /// Every plan ChatGPT sells.
    pub const ALL: &'static [Self] = &[
        Self::Free,
        Self::Plus,
        Self::Pro,
        Self::Team,
        Self::Business,
        Self::Enterprise,
        Self::Edu,
    ];

    /// The plan `subscription` spells, when it is one ChatGPT sells. The
    /// spelling is exact: `Pro` names none.
    pub fn named(subscription: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|plan| plan.name() == subscription)
    }

    /// The plan as the id token spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Plus => "plus",
            Self::Pro => "pro",
            Self::Team => "team",
            Self::Business => "business",
            Self::Enterprise => "enterprise",
            Self::Edu => "edu",
        }
    }

    /// The plan as ChatGPT names it, such as `ChatGPT Pro`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Free => "ChatGPT Free",
            Self::Plus => "ChatGPT Plus",
            Self::Pro => "ChatGPT Pro",
            Self::Team => "ChatGPT Team",
            Self::Business => "ChatGPT Business",
            Self::Enterprise => "ChatGPT Enterprise",
            Self::Edu => "ChatGPT Edu",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_plan_is_named_as_chatgpt_names_it() {
        for (subscription, expected) in [
            ("plus", Some("ChatGPT Plus")),
            ("pro", Some("ChatGPT Pro")),
            ("team", Some("ChatGPT Team")),
            ("enterprise", Some("ChatGPT Enterprise")),
            ("Pro", None),
            ("", None),
            ("unlimited", None),
        ] {
            assert_eq!(
                Plan::named(subscription).map(Plan::label),
                expected,
                "{subscription:?}"
            );
        }
        for plan in Plan::ALL {
            assert_eq!(Plan::named(plan.name()), Some(*plan));
        }
    }
}
