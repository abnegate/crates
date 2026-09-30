/// Whether the turn a tool serves is offered [`WAIT_FOR`](crate::tool::job::WAIT_FOR).
///
/// A coding agent that runs its own loop never is: a wait parks only this
/// crate's loop. It is served the same tools as a turn that is, so what one of
/// them says about waiting is said for one kind of turn or the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WaitFor {
    /// The turn can call `wait_for`, and is told to in the words it always was.
    Offered,
    /// The turn cannot, so each instruction to call `wait_for` says it needs
    /// the tool.
    Withheld,
}

impl WaitFor {
    /// Every form, for a reader that accepts whichever one it was handed.
    pub const ALL: &[Self] = &[Self::Offered, Self::Withheld];

    /// What follows each instruction to call `wait_for`: nothing on a turn
    /// that has the tool, and on one that does not, that the instruction holds
    /// only where it does.
    pub const fn condition(self) -> &'static str {
        match self {
            Self::Offered => "",
            Self::Withheld => " when you have that tool",
        }
    }
}
