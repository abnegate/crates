/// The consequence class of a tool call, declared by the tool itself.
///
/// Ordered by what a wrong call costs, so the gate is a comparison against one
/// threshold rather than a list of tool names that has to be kept in step with
/// the catalog by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Tier {
    /// Observes and changes nothing, so it never waits and may share a batch.
    Read,
    /// Changes durable state through an audited workspace API, where the
    /// operator can see what happened and put it back.
    Write,
    /// Writes files or runs commands in the runtime the agent is hosted in.
    /// Nothing records what it did beyond the tool result.
    Host,
    /// Reaches a person, or a system outside this workspace. What it sends
    /// cannot be recalled.
    Outward,
}

/// The lowest tier a caller confirms with the user before running.
pub const CONFIRMED_FROM: Tier = Tier::Host;

impl Tier {
    /// Whether a call at this tier changes anything.
    ///
    /// Read-only calls may run together in one batch; anything else keeps the
    /// batch sequential so later reads see earlier writes.
    pub const fn mutating(self) -> bool {
        !matches!(self, Self::Read)
    }

    /// Whether a call at this tier is put to the user before it runs.
    pub const fn confirmed(self) -> bool {
        (self as u8) >= (CONFIRMED_FROM as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_reads_are_free_of_consequence() {
        assert!(!Tier::Read.mutating());
        for tier in [Tier::Write, Tier::Host, Tier::Outward] {
            assert!(tier.mutating(), "{tier:?}");
        }
    }

    /// An audited workspace write is undoable and stays unconfirmed; touching
    /// the host or leaving the workspace is not, and does.
    #[test]
    fn confirmation_starts_at_the_host_and_covers_everything_above_it() {
        assert!(!Tier::Read.confirmed());
        assert!(!Tier::Write.confirmed());
        assert!(Tier::Host.confirmed());
        assert!(Tier::Outward.confirmed());
    }

    /// The threshold is a comparison, so the ordering is what makes it correct.
    #[test]
    fn the_tiers_are_ordered_by_what_a_wrong_call_costs() {
        assert!(Tier::Read < Tier::Write);
        assert!(Tier::Write < Tier::Host);
        assert!(Tier::Host < Tier::Outward);
        assert_eq!(CONFIRMED_FROM, Tier::Host);
    }

    /// Every tier above the threshold has to be confirmed, whatever is added
    /// later: a new top variant that is not is the bug this catches.
    #[test]
    fn no_tier_above_the_threshold_escapes_confirmation() {
        for tier in [Tier::Read, Tier::Write, Tier::Host, Tier::Outward] {
            assert_eq!(tier.confirmed(), tier >= CONFIRMED_FROM, "{tier:?}");
        }
    }
}
