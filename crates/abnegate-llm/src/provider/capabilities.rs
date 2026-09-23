/// What a provider supports beyond returning a completion.
///
/// A caller that needs one of these — a schema-constrained answer, a per-tool
/// permission prompt, a cost line for a budget — states the requirement and
/// lets [`Router`](super::Router) route around the providers that cannot meet
/// it, rather than matching on provider names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub structured_output: bool,
    pub tool_permissions: bool,
    pub custom_instructions: bool,
    pub streaming_events: bool,
    pub cost_reporting: bool,
}

impl Capabilities {
    /// Asks for nothing, which every provider satisfies.
    pub const NONE: Self = Self {
        structured_output: false,
        tool_permissions: false,
        custom_instructions: false,
        streaming_events: false,
        cost_reporting: false,
    };

    /// Asks for everything, which only a fully featured provider satisfies.
    pub const ALL: Self = Self {
        structured_output: true,
        tool_permissions: true,
        custom_instructions: true,
        streaming_events: true,
        cost_reporting: true,
    };

    /// Whether these capabilities cover everything `required` asks for.
    pub fn satisfies(self, required: Self) -> bool {
        (!required.structured_output || self.structured_output)
            && (!required.tool_permissions || self.tool_permissions)
            && (!required.custom_instructions || self.custom_instructions)
            && (!required.streaming_events || self.streaming_events)
            && (!required.cost_reporting || self.cost_reporting)
    }
}

#[cfg(test)]
mod tests {
    use super::Capabilities;

    #[test]
    fn a_provider_supports_nothing_until_it_says_otherwise() {
        assert_eq!(Capabilities::default(), Capabilities::NONE);
    }

    #[test]
    fn everything_satisfies_a_requirement_for_nothing() {
        assert!(Capabilities::NONE.satisfies(Capabilities::NONE));
        assert!(Capabilities::ALL.satisfies(Capabilities::NONE));
        assert!(Capabilities::ALL.satisfies(Capabilities::ALL));
    }

    #[test]
    fn a_missing_capability_fails_the_requirement() {
        let required = Capabilities {
            structured_output: true,
            ..Capabilities::NONE
        };

        assert!(!Capabilities::NONE.satisfies(required));
        assert!(
            Capabilities {
                structured_output: true,
                ..Capabilities::NONE
            }
            .satisfies(required)
        );
    }

    #[test]
    fn a_capability_not_asked_for_does_not_disqualify_a_provider() {
        let required = Capabilities {
            cost_reporting: true,
            ..Capabilities::NONE
        };

        assert!(
            Capabilities {
                cost_reporting: true,
                streaming_events: false,
                ..Capabilities::ALL
            }
            .satisfies(required)
        );
    }

    #[test]
    fn every_capability_is_checked_on_its_own() {
        for required in [
            Capabilities {
                structured_output: true,
                ..Capabilities::NONE
            },
            Capabilities {
                tool_permissions: true,
                ..Capabilities::NONE
            },
            Capabilities {
                custom_instructions: true,
                ..Capabilities::NONE
            },
            Capabilities {
                streaming_events: true,
                ..Capabilities::NONE
            },
            Capabilities {
                cost_reporting: true,
                ..Capabilities::NONE
            },
        ] {
            assert!(!Capabilities::NONE.satisfies(required), "{required:?}");
            assert!(Capabilities::ALL.satisfies(required), "{required:?}");
        }
    }
}
