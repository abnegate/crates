#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! An RFC 5545 `RRULE` subset evaluated on an IANA timezone's wall clock.
//!
//! [`Recurrence::parse`] reads `FREQ` (hourly, daily, weekly or monthly),
//! `INTERVAL`, `BYDAY`, `BYHOUR`, `BYMINUTE`, `BYMONTHDAY`, `UNTIL` and
//! `COUNT`. Every other clause, and every clause the rule's frequency never
//! reads, is refused with an [`Error`] that names it rather than ignored.
//!
//! [`Recurrence::next_after`] answers with the next firing, reading every
//! clause on a [`Timezone`]'s wall clock so nine o'clock stays nine o'clock
//! across a daylight-saving change:
//!
//! - A time the clocks skip fires at the first whole minute after the gap,
//!   the instant the clocks change wherever both offsets are whole minutes.
//! - A time the clocks show twice fires at the first of the two, once.
//! - An hourly rule counts elapsed UTC hours, which no clock change moves.
//! - `BYMONTHDAY=31` skips the months without a 31st, while a day inherited
//!   from the anchor clamps to the month's last day.
//! - A rule nothing can satisfy answers `None` after a bounded search.
//!
//! [`Recurrence::period`] is the shortest gap between two firings, so a host
//! that holds rules to a minimum cadence applies its own floor to it.
//!
//! ```
//! use abnegate_recurrence::Recurrence;
//! use abnegate_recurrence::Timezone;
//! use chrono::DateTime;
//! use chrono::TimeDelta;
//! use chrono::Utc;
//!
//! let rule = Recurrence::parse("RRULE:FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9").expect("a rule");
//! let london = Timezone::parse("Europe/London").expect("a timezone");
//! let instant = |text: &str| text.parse::<DateTime<Utc>>().expect("an instant");
//!
//! let anchor = instant("2027-03-22T09:00:00Z");
//! let friday = rule.next_after(anchor, anchor, 1, london);
//! assert_eq!(friday, Some(instant("2027-03-26T09:00:00Z")));
//!
//! let monday = rule.next_after(anchor, instant("2027-03-26T09:00:00Z"), 2, london);
//! assert_eq!(monday, Some(instant("2027-03-29T08:00:00Z")));
//!
//! assert_eq!(rule.period(), TimeDelta::days(3));
//! ```

mod clause;
mod day;
mod error;
mod frequency;
mod recurrence;
mod timezone;

pub use clause::Clause;
pub use error::Error;
pub use error::Result;
pub use frequency::Frequency;
pub use recurrence::Recurrence;
pub use timezone::Timezone;
