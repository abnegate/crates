/// What a provider supports beyond returning a completion.
///
/// A caller that needs one of these — a schema-constrained answer, a per-tool
/// permission prompt, a cost line for a budget — states the requirement and
/// lets [`Router`](super::Router) route around the providers that cannot meet
/// it, rather than matching on provider names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
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

    /// These capabilities with schema-constrained output added.
    pub const fn with_structured_output(mut self) -> Self {
        self.structured_output = true;
        self
    }

    /// These capabilities with per-tool permission prompts added.
    pub const fn with_tool_permissions(mut self) -> Self {
        self.tool_permissions = true;
        self
    }

    /// These capabilities with custom instructions added.
    pub const fn with_custom_instructions(mut self) -> Self {
        self.custom_instructions = true;
        self
    }

    /// These capabilities with streamed events added.
    pub const fn with_streaming_events(mut self) -> Self {
        self.streaming_events = true;
        self
    }

    /// These capabilities with cost reporting added.
    pub const fn with_cost_reporting(mut self) -> Self {
        self.cost_reporting = true;
        self
    }

    /// What both sets support, which is what a caller can rely on when either
    /// one may end up serving the request.
    pub fn intersection(self, other: Self) -> Self {
        Self {
            structured_output: self.structured_output && other.structured_output,
            tool_permissions: self.tool_permissions && other.tool_permissions,
            custom_instructions: self.custom_instructions && other.custom_instructions,
            streaming_events: self.streaming_events && other.streaming_events,
            cost_reporting: self.cost_reporting && other.cost_reporting,
        }
    }

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
    fn a_set_is_built_one_capability_at_a_time() {
        const STREAMING: Capabilities = Capabilities::NONE.with_streaming_events();
        const { assert!(STREAMING.streaming_events && !STREAMING.structured_output) };
        assert_eq!(
            Capabilities::NONE
                .with_structured_output()
                .with_tool_permissions()
                .with_custom_instructions()
                .with_streaming_events()
                .with_cost_reporting(),
            Capabilities::ALL
        );
    }

    #[test]
    fn an_intersection_keeps_only_what_both_support() {
        let structured = Capabilities {
            structured_output: true,
            cost_reporting: true,
            ..Capabilities::NONE
        };
        let costed = Capabilities {
            cost_reporting: true,
            ..Capabilities::NONE
        };

        assert_eq!(structured.intersection(costed), costed);
        assert_eq!(Capabilities::ALL.intersection(structured), structured);
        assert_eq!(
            Capabilities::NONE.intersection(Capabilities::ALL),
            Capabilities::NONE
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
