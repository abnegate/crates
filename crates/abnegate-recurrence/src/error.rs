//! The result of every fallible operation in this crate.

use crate::clause::Clause;
use crate::frequency::Frequency;

/// The result of every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a rule or a timezone name was refused.
///
/// Each refusal names the clause it read and what it wanted instead, so the
/// text is fit to show whoever wrote the rule. A variant may gain a field in a
/// minor release, so a pattern outside this crate ends in `..`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The rule was blank, or only its `RRULE:` prefix.
    #[error("An rrule cannot be empty. Give at least FREQ.")]
    Empty,
    /// A part of the rule had no `=` between its name and its value.
    #[error("\"{part}\" is not NAME=VALUE.")]
    #[non_exhaustive]
    Malformed {
        /// The part as the rule spelled it, untrimmed.
        part: String,
    },
    /// A clause outside the accepted subset, which is refused rather than
    /// ignored.
    #[error(
        "{name} is not a clause this build accepts. Use FREQ, INTERVAL, BYDAY, BYHOUR, \
         BYMINUTE, BYMONTHDAY, UNTIL or COUNT."
    )]
    #[non_exhaustive]
    UnknownClause {
        /// The clause name, trimmed and upper-cased.
        name: String,
    },
    /// `FREQ` named a cadence other than hourly, daily, weekly or monthly.
    #[error(
        "FREQ={value} is not supported. Use HOURLY, DAILY, WEEKLY or MONTHLY; HOURLY is the \
         finest cadence a schedule can have."
    )]
    #[non_exhaustive]
    UnknownFrequency {
        /// The value as the rule spelled it, trimmed.
        value: String,
    },
    /// `INTERVAL` was not a whole number of at least one.
    #[error("INTERVAL={value} must be a positive whole number.")]
    #[non_exhaustive]
    Interval {
        /// The value as the rule spelled it, trimmed.
        value: String,
    },
    /// `COUNT` was not a whole number of at least one.
    #[error("COUNT={value} must be a positive whole number.")]
    #[non_exhaustive]
    Count {
        /// The value as the rule spelled it, trimmed.
        value: String,
    },
    /// `UNTIL` was not a UTC timestamp in RFC 5545's basic form.
    #[error("UNTIL={value} must read as 20260914T090000Z.")]
    #[non_exhaustive]
    Until {
        /// The value as the rule spelled it, trimmed.
        value: String,
    },
    /// One item of a list clause was outside what that clause takes.
    #[error("{clause}={value} is not a value {clause} takes.")]
    #[non_exhaustive]
    Value {
        /// The clause the item belongs to.
        clause: Clause,
        /// The item, trimmed.
        value: String,
    },
    /// A list clause had no items once its blanks were dropped.
    #[error("{clause} was given no values.")]
    #[non_exhaustive]
    Blank {
        /// The empty clause.
        clause: Clause,
    },
    /// The rule had no `FREQ`.
    #[error("An rrule must state FREQ.")]
    MissingFrequency,
    /// The rule had both `UNTIL` and `COUNT`.
    #[error("Give UNTIL or COUNT, not both.")]
    UntilAndCount,
    /// The rule named clauses its frequency never reads, which would otherwise
    /// fire at times nobody asked for.
    #[error(
        "{frequency} does not use {}. Drop the clause, or state the FREQ that reads it: \
         BYDAY needs WEEKLY, BYMONTHDAY needs MONTHLY, and BYHOUR and BYMINUTE need DAILY, \
         WEEKLY or MONTHLY.",
        names(.clauses)
    )]
    #[non_exhaustive]
    Unread {
        /// The rule's frequency.
        frequency: Frequency,
        /// The unread clauses, in the order `BYDAY`, `BYMONTHDAY`, `BYHOUR`,
        /// `BYMINUTE`.
        clauses: Vec<Clause>,
    },
    /// A name the tz database does not hold.
    #[error(
        "\"{name}\" is not an IANA timezone name. Use one such as Europe/London or \
         Pacific/Auckland."
    )]
    #[non_exhaustive]
    Timezone {
        /// The name as given, untrimmed.
        name: String,
    },
}

fn names(clauses: &[Clause]) -> String {
    clauses
        .iter()
        .map(|clause| clause.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
