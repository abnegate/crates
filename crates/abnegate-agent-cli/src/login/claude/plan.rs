/// A Claude plan a sign-in can be on, as Claude's token endpoint and
/// `claude auth status` both spell it in `subscription`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Plan {
    /// Claude Max, spelled `max`.
    Max,
    /// Claude Pro, spelled `pro`.
    Pro,
    /// Claude Team, spelled `team`.
    Team,
    /// Claude Enterprise, spelled `enterprise`.
    Enterprise,
}

impl Plan {
    /// Every plan the claude CLI names.
    pub const ALL: &'static [Self] = &[Self::Max, Self::Pro, Self::Team, Self::Enterprise];

    /// The plan `subscription` spells, when it is one the claude CLI names.
    /// The spelling is exact: `Team` names none.
    pub fn named(subscription: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|plan| plan.name() == subscription)
    }

    /// The plan as `subscription` spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Max => "max",
            Self::Pro => "pro",
            Self::Team => "team",
            Self::Enterprise => "enterprise",
        }
    }

    /// The plan as the claude CLI shows it, such as `Claude Max`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Max => "Claude Max",
            Self::Pro => "Claude Pro",
            Self::Team => "Claude Team",
            Self::Enterprise => "Claude Enterprise",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_plan_is_named_as_the_cli_names_it() {
        for (subscription, expected) in [
            ("max", Some("Claude Max")),
            ("pro", Some("Claude Pro")),
            ("team", Some("Claude Team")),
            ("enterprise", Some("Claude Enterprise")),
            ("free", None),
            ("", None),
            ("Team", None),
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
