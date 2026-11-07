//! The wall clock a rule is read on, and the instants its times name.

use crate::error::Error;
use crate::error::Result;
use chrono::DateTime;
use chrono::FixedOffset;
use chrono::LocalResult;
use chrono::NaiveDateTime;
use chrono::Offset;
use chrono::TimeDelta;
use chrono::TimeZone as _;
use chrono::Timelike;
use chrono::Utc;
use chrono_tz::Tz;

/// How far a skipped time is carried before it is given up on: longer than the
/// longest gap the tz database holds, the whole day Samoa skipped in 2011.
const LONGEST_GAP: TimeDelta = TimeDelta::hours(25);

/// An IANA timezone, read from the tz database.
///
/// Two kinds of wall-clock time have no single instant, and both are settled
/// the same way every time by [`Timezone::instant`]:
///
/// - A time the clocks skip, in the hour a spring-forward removes, moves
///   forward to the first whole minute the clocks show after the gap, which
///   is the instant they change wherever both offsets are whole minutes.
///   01:30 on the morning London skips is 02:00 BST.
/// - A time the clocks show twice, in the hour a fall-back repeats, is the
///   first of the two. 01:30 on the morning London repeats is 01:30 BST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timezone(Tz);

impl Timezone {
    /// Coordinated Universal Time, which never skips or repeats a time.
    pub const UTC: Self = Self(Tz::UTC);

    /// Reads an IANA name, such as `Europe/London`, ignoring surrounding
    /// whitespace. The name is matched exactly as the tz database spells it.
    ///
    /// # Errors
    ///
    /// [`Error::Timezone`] when the tz database holds no such name.
    pub fn parse(name: &str) -> Result<Self> {
        name.trim()
            .parse::<Tz>()
            .map(Self)
            .map_err(|_| Error::Timezone {
                name: name.to_string(),
            })
    }

    /// The IANA name, such as `Pacific/Auckland`.
    pub fn name(self) -> &'static str {
        self.0.name()
    }

    /// The offset from UTC this timezone's clocks show at `instant`.
    pub fn offset(self, instant: DateTime<Utc>) -> FixedOffset {
        self.0.offset_from_utc_datetime(&instant.naive_utc()).fix()
    }

    /// What this timezone's clocks show at `instant`.
    ///
    /// # Panics
    ///
    /// When the reading falls outside what [`NaiveDateTime`] can represent:
    /// within a day of [`DateTime::<Utc>::MAX_UTC`] in a timezone ahead of
    /// UTC, or of [`DateTime::<Utc>::MIN_UTC`] in one behind it.
    pub fn local(self, instant: DateTime<Utc>) -> NaiveDateTime {
        instant.with_timezone(&self.0).naive_local()
    }

    /// The instant this timezone's clocks show `local`.
    ///
    /// A time the clocks skip answers with the first whole minute they show
    /// after the gap. That is the instant they change, except where an offset
    /// carries seconds, as historical local mean time did, and there it is
    /// under a minute later: Monrovia's clocks moved from 00:00 to 00:44:30 on
    /// 7 January 1972, and 00:00 that day answers 00:45:00.
    ///
    /// A time they show twice answers with the first of the two. `None` only
    /// when no shown minute follows within a day and an hour, or none follows
    /// before the end of the calendar.
    pub fn instant(self, local: NaiveDateTime) -> Option<DateTime<Utc>> {
        let mut shown = local;
        while shown - local <= LONGEST_GAP {
            match self.0.from_local_datetime(&shown) {
                LocalResult::Single(instant) | LocalResult::Ambiguous(instant, _) => {
                    return Some(instant.with_timezone(&Utc));
                }
                LocalResult::None => {
                    shown = shown
                        .checked_add_signed(TimeDelta::minutes(1))?
                        .with_second(0)?
                        .with_nanosecond(0)?;
                }
            }
        }
        None
    }
}
