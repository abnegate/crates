//! One accepted rule, and the arithmetic that turns it into the next instant.

use crate::clause::Clause;
use crate::day::Day;
use crate::error::Error;
use crate::error::Result;
use crate::frequency::Frequency;
use crate::frequency::SHORTEST_MONTH_DAYS;
use crate::timezone::Timezone;
use chrono::DateTime;
use chrono::Datelike;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::TimeDelta;
use chrono::Timelike;
use chrono::Utc;
use chrono::Weekday;

#[cfg(test)]
mod tests;

/// How many periods a search walks before it gives up. As monthly periods
/// that is at least 125 years, which clears every leap-year case, a skipped
/// century leap year included, so a rule whose clauses can never all hold,
/// such as the 31st of every February, answers `None` instead of walking
/// forward for ever.
const MAXIMUM_PERIODS: u32 = 1_500;

const PREFIX: &str = "RRULE:";

/// `UNTIL` in the one form RFC 5545 gives it for a UTC rule. The trailing `Z`
/// is a literal rather than an offset chrono can read, so the stamp parses
/// naive and is declared UTC.
const UNTIL_FORMAT: &str = "%Y%m%dT%H%M%SZ";

const MINUTES_PER_HOUR: i64 = 60;
const MONTHS_PER_YEAR: i64 = 12;
const LAST_HOUR: u32 = 23;
const LAST_MINUTE: u32 = 59;
const FIRST_MONTH_DAY: u32 = 1;
const LAST_MONTH_DAY: u32 = 31;

/// One accepted RFC 5545 `RRULE`.
///
/// The subset is deliberate rather than partial: `FREQ` (hourly, daily,
/// weekly or monthly), `INTERVAL`, `BYDAY`, `BYHOUR`, `BYMINUTE`,
/// `BYMONTHDAY`, `UNTIL` and `COUNT`. Any other clause, and any clause the
/// rule's frequency never reads, is refused rather than ignored, because a
/// rule that silently drops a clause fires at a time nobody asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recurrence {
    frequency: Frequency,
    interval: u32,
    by_day: Vec<Weekday>,
    by_hour: Vec<u32>,
    by_minute: Vec<u32>,
    by_month_day: Vec<u32>,
    until: Option<DateTime<Utc>>,
    count: Option<u32>,
}

impl Recurrence {
    /// Reads an `RRULE` line, with or without its `RRULE:` prefix.
    ///
    /// The `RRULE:` prefix is matched exactly as written, in upper case, so
    /// `rrule:FREQ=DAILY` is refused as a clause named `RRULE:FREQ`. Clause
    /// names, and the `FREQ` and `BYDAY` values, are read in any case;
    /// `UNTIL` must be RFC 5545's UTC form exactly as written, such as
    /// `20260914T090000Z`, with its `T` and `Z` in upper case. Names, values
    /// and list items are read around any whitespace; blank parts and blank
    /// list items are skipped, list items are sorted and deduplicated, and a
    /// clause given twice keeps its later value.
    ///
    /// Faults within clauses are reported in rule order; a missing `FREQ`,
    /// `UNTIL` given with `COUNT`, and clauses the frequency never reads are
    /// checked after every clause has been read.
    ///
    /// # Errors
    ///
    /// An [`Error`] naming the clause that was refused and what it wanted.
    pub fn parse(rule: &str) -> Result<Self> {
        let body = rule
            .trim()
            .strip_prefix(PREFIX)
            .unwrap_or_else(|| rule.trim());
        if body.is_empty() {
            return Err(Error::Empty);
        }

        let mut frequency = None;
        let mut interval = 1;
        let mut by_day = Vec::new();
        let mut by_hour = Vec::new();
        let mut by_minute = Vec::new();
        let mut by_month_day = Vec::new();
        let mut until = None;
        let mut count = None;

        for part in body.split(';').filter(|part| !part.trim().is_empty()) {
            let (name, value) = part.split_once('=').ok_or_else(|| Error::Malformed {
                part: part.to_string(),
            })?;
            let value = value.trim();
            let name = name.trim().to_ascii_uppercase();
            match Clause::parse(&name) {
                Some(Clause::Frequency) => {
                    frequency = Some(Frequency::parse(&value.to_ascii_uppercase()).ok_or_else(
                        || Error::UnknownFrequency {
                            value: value.to_string(),
                        },
                    )?);
                }
                Some(Clause::Interval) => {
                    interval = positive(value).ok_or_else(|| Error::Interval {
                        value: value.to_string(),
                    })?;
                }
                Some(Clause::Day) => {
                    by_day = list(
                        value,
                        |raw| Day::parse(&raw.to_ascii_uppercase()),
                        Clause::Day,
                    )?
                    .into_iter()
                    .map(|Day(day)| day)
                    .collect();
                }
                Some(Clause::Hour) => {
                    by_hour = list(value, |raw| bounded(raw, 0, LAST_HOUR), Clause::Hour)?;
                }
                Some(Clause::Minute) => {
                    by_minute = list(value, |raw| bounded(raw, 0, LAST_MINUTE), Clause::Minute)?;
                }
                Some(Clause::MonthDay) => {
                    by_month_day = list(
                        value,
                        |raw| bounded(raw, FIRST_MONTH_DAY, LAST_MONTH_DAY),
                        Clause::MonthDay,
                    )?;
                }
                Some(Clause::Until) => until = Some(timestamp(value)?),
                Some(Clause::Count) => {
                    count = Some(positive(value).ok_or_else(|| Error::Count {
                        value: value.to_string(),
                    })?);
                }
                None => return Err(Error::UnknownClause { name }),
            }
        }

        let frequency = frequency.ok_or(Error::MissingFrequency)?;
        if until.is_some() && count.is_some() {
            return Err(Error::UntilAndCount);
        }
        let recurrence = Self {
            frequency,
            interval,
            by_day,
            by_hour,
            by_minute,
            by_month_day,
            until,
            count,
        };
        recurrence.clauses_the_frequency_uses()?;
        Ok(recurrence)
    }

    /// Refuses a clause this frequency's arithmetic never reads.
    ///
    /// `FREQ=HOURLY;BYHOUR=9` reads as "every day at nine" and would fire
    /// twenty-four times a day; `FREQ=DAILY;BYDAY=MO,FR` reads as "Mondays and
    /// Fridays" and would fire every day. An hourly period is its own firing
    /// and reads none of the `BY` clauses; a day reads the hour and the
    /// minute; a week reads those and `BYDAY`; a month reads those and
    /// `BYMONTHDAY`.
    fn clauses_the_frequency_uses(&self) -> Result<()> {
        let clauses: Vec<Clause> = [
            (
                Clause::Day,
                !self.by_day.is_empty(),
                self.frequency != Frequency::Weekly,
            ),
            (
                Clause::MonthDay,
                !self.by_month_day.is_empty(),
                self.frequency != Frequency::Monthly,
            ),
            (
                Clause::Hour,
                !self.by_hour.is_empty(),
                self.frequency == Frequency::Hourly,
            ),
            (
                Clause::Minute,
                !self.by_minute.is_empty(),
                self.frequency == Frequency::Hourly,
            ),
        ]
        .into_iter()
        .filter(|(_, given, unread)| *given && *unread)
        .map(|(clause, _, _)| clause)
        .collect();

        if clauses.is_empty() {
            return Ok(());
        }
        Err(Error::Unread {
            frequency: self.frequency,
            clauses,
        })
    }

    /// The shortest gap between two firings.
    ///
    /// Shortest rather than average: `FREQ=DAILY;BYHOUR=0,1;BYMINUTE=0,30`
    /// fires four times a day, so its firings average six hours apart, but
    /// three of its gaps are half an hour. The gap is measured where it is
    /// smallest, between two times on one date and out of a period's last
    /// date into the next period's first, with a month counted at
    /// twenty-eight days. A host that holds rules to a minimum cadence
    /// compares this against its own floor.
    ///
    /// An hour, minute or date the rule leaves unstated is the anchor's, which
    /// is one value rather than none, and which value it is never matters
    /// here, because every gap is a difference. A period longer than
    /// [`TimeDelta`] can hold, which only a monthly `INTERVAL` in the billions
    /// reaches, reads as [`TimeDelta::MAX`].
    ///
    /// The gap is measured on the wall clock. A daylight-saving change can
    /// bring two firings closer in elapsed time by the size of that change:
    /// `FREQ=DAILY;BYHOUR=0,2` fires at 00:00 and 02:00, two hours apart on
    /// the clock, but on the morning London springs forward those are one
    /// elapsed hour apart. A host that bounds elapsed time between firings
    /// allows for the largest change its timezones make.
    pub fn period(&self) -> TimeDelta {
        let whole = self
            .frequency
            .shortest()
            .num_milliseconds()
            .checked_mul(i64::from(self.interval))
            .and_then(TimeDelta::try_milliseconds)
            .unwrap_or(TimeDelta::MAX);
        if self.frequency == Frequency::Hourly {
            return whole;
        }
        let times = self.times_of_day();
        let span = match (times.first(), times.last()) {
            (Some(first), Some(last)) => last - first,
            _ => 0,
        };
        let across = self.shortest_gap_between_dates(whole) - TimeDelta::minutes(span);
        times
            .windows(2)
            .map(|pair| TimeDelta::minutes(pair[1] - pair[0]))
            .min()
            .map_or(across, |within_a_date| within_a_date.min(across))
    }

    /// The minutes past midnight one selected date offers, in order, with
    /// midnight standing in for an hour or a minute the rule does not state.
    fn times_of_day(&self) -> Vec<i64> {
        let hours = if self.by_hour.is_empty() {
            vec![0]
        } else {
            self.by_hour.clone()
        };
        let minutes = if self.by_minute.is_empty() {
            vec![0]
        } else {
            self.by_minute.clone()
        };
        let mut times: Vec<i64> = hours
            .iter()
            .flat_map(|hour| {
                minutes
                    .iter()
                    .map(move |minute| i64::from(*hour) * MINUTES_PER_HOUR + i64::from(*minute))
            })
            .collect();
        times.sort_unstable();
        times.dedup();
        times
    }

    /// The shortest gap between two dates the rule selects, counting the step
    /// out of one period's last date into the next period's first.
    ///
    /// A frequency that names no dates inside its period selects exactly one,
    /// so the gap is the period itself. Two distinct dates are never less than
    /// a day apart, and a month measured at twenty-eight days undercounts the
    /// step out of a longer one, so the gap is never read as less than a day.
    fn shortest_gap_between_dates(&self, whole: TimeDelta) -> TimeDelta {
        let mut days: Vec<i64> = match self.frequency {
            Frequency::Weekly => self
                .by_day
                .iter()
                .map(|day| i64::from(day.num_days_from_monday()))
                .collect(),
            Frequency::Monthly => self
                .by_month_day
                .iter()
                .map(|day| i64::from(*day))
                .collect(),
            Frequency::Hourly | Frequency::Daily => Vec::new(),
        };
        days.sort_unstable();
        days.dedup();
        let (Some(first), Some(last)) = (days.first(), days.last()) else {
            return whole;
        };
        let Some(within_a_period) = days.windows(2).map(|pair| pair[1] - pair[0]).min() else {
            return whole;
        };
        let into_the_next = whole.num_days() - (last - first);
        TimeDelta::days(within_a_period.min(into_the_next).max(1))
    }

    /// Whether a rule anchored at `instant` would fire there itself, judged to
    /// the minute on `timezone`'s wall clock.
    ///
    /// Asked of the search rather than of the clauses, so a time the clocks
    /// skip is judged where the rule actually lands it, and the second pass of
    /// a repeated hour is not a time the rule fires at.
    ///
    /// # Panics
    ///
    /// When `instant` falls in the first minute [`DateTime<Utc>`] can
    /// represent, or when [`Timezone::local`] panics for `instant` on
    /// `timezone`.
    pub fn fires_at(&self, instant: DateTime<Utc>, timezone: Timezone) -> bool {
        let minute = |at: DateTime<Utc>| at.with_second(0).and_then(|at| at.with_nanosecond(0));
        let Some(start) = minute(instant) else {
            return false;
        };
        self.next_after(instant, start - TimeDelta::minutes(1), 0, timezone)
            .and_then(minute)
            == Some(start)
    }

    /// The first firing strictly after `after`, or `None` once the rule is
    /// spent.
    ///
    /// `anchor` is the first firing: it supplies whatever the rule does not
    /// say, so `FREQ=WEEKLY` alone repeats on the anchor's own weekday and
    /// time, and an `INTERVAL` counts from the anchor's period. `fired` is how
    /// many firings have already happened, which is what `COUNT` is measured
    /// against. `UNTIL` includes the instant it names.
    ///
    /// Every clause is read on `timezone`'s wall clock, and so is the anchor,
    /// so nine o'clock stays nine o'clock there across a daylight-saving
    /// change; a skipped or repeated time is settled as
    /// [`Timezone::instant`] settles it. An hourly rule is the exception: it
    /// counts elapsed hours, which no clock change moves, so it is read in UTC
    /// whatever the timezone.
    ///
    /// A day `BYMONTHDAY` names is that date or no date: `BYMONTHDAY=31` skips
    /// every month without a 31st, as RFC 5545 does. A day inherited from the
    /// anchor asks for "this point in each month" instead, so it clamps to the
    /// month's last day, and a rule anchored on 31 January fires on
    /// 28 February. A rule whose clauses can never all hold answers `None`
    /// after a bounded search rather than walking forward for ever, and so
    /// does a rule whose next period lies past the end of the calendar.
    ///
    /// An hourly firing keeps the anchor's minute and the anchor's fraction of
    /// a second, dropping only its whole seconds, so an anchor at 09:17:42.5
    /// fires at 10:17:00.5; every other firing falls on a whole minute.
    ///
    /// `anchor` is the same instant for the rule's whole life: the day a
    /// monthly rule clamps from, the phase an `INTERVAL` counts from, and the
    /// first of the firings `fired` counts. A rule anchored on 31 January
    /// fires on 28 February and then 31 March, where one re-anchored on
    /// 28 February would fire on the 28th for ever after.
    ///
    /// The anchor is itself the first firing, so a host takes it as such,
    /// checking it with [`Recurrence::fires_at`], and asks for each later one
    /// with `after` at or past the anchor. An `after` earlier than `anchor`
    /// reads the periods before the anchor as well, in the anchor's phase, so
    /// it can answer with a firing before the anchor: a daily rule anchored on
    /// 14 September and asked after noon on 1 September answers 2 September.
    /// [`Recurrence::fires_at`] relies on this, asking from the minute before
    /// the anchor's own.
    ///
    /// Finding the period `after` falls in takes one step per period between
    /// `anchor` and `after`, so the cost grows with that distance divided by
    /// the period.
    ///
    /// # Panics
    ///
    /// When [`Timezone::local`] panics for `anchor` or `after` on `timezone`.
    pub fn next_after(
        &self,
        anchor: DateTime<Utc>,
        after: DateTime<Utc>,
        fired: u32,
        timezone: Timezone,
    ) -> Option<DateTime<Utc>> {
        if self.count.is_some_and(|count| fired >= count) {
            return None;
        }
        let timezone = if self.frequency == Frequency::Hourly {
            Timezone::UTC
        } else {
            timezone
        };
        let anchor_local = timezone.local(anchor);

        let hours = if self.by_hour.is_empty() {
            vec![anchor_local.hour()]
        } else {
            self.by_hour.clone()
        };
        let minutes = if self.by_minute.is_empty() {
            vec![anchor_local.minute()]
        } else {
            self.by_minute.clone()
        };

        let mut period = self.period_start(anchor_local, timezone.local(after));
        for _ in 0..MAXIMUM_PERIODS {
            for local in self.candidates(period, anchor_local, &hours, &minutes) {
                let Some(candidate) = timezone.instant(local) else {
                    continue;
                };
                if candidate <= after {
                    continue;
                }
                if self.until.is_some_and(|until| candidate > until) {
                    return None;
                }
                return Some(candidate);
            }
            period = self.advance(period)?;
        }
        None
    }

    /// The start of the period `after` falls in, walked from the anchor so an
    /// `INTERVAL` keeps the anchor's phase.
    fn period_start(&self, anchor: NaiveDateTime, after: NaiveDateTime) -> NaiveDateTime {
        let mut period = anchor;
        while period > after {
            let Some(back) = self.retreat(period) else {
                break;
            };
            period = back;
        }
        while self.advance(period).is_some_and(|next| next <= after) {
            let Some(next) = self.advance(period) else {
                break;
            };
            period = next;
        }
        period
    }

    fn advance(&self, from: NaiveDateTime) -> Option<NaiveDateTime> {
        self.step(from, 1)
    }

    fn retreat(&self, from: NaiveDateTime) -> Option<NaiveDateTime> {
        self.step(from, -1)
    }

    fn step(&self, from: NaiveDateTime, direction: i64) -> Option<NaiveDateTime> {
        let interval = i64::from(self.interval) * direction;
        match self.frequency {
            Frequency::Hourly => from.checked_add_signed(TimeDelta::try_hours(interval)?),
            Frequency::Daily => from.checked_add_signed(TimeDelta::try_days(interval)?),
            Frequency::Weekly => from.checked_add_signed(TimeDelta::try_weeks(interval)?),
            Frequency::Monthly => months(from, interval),
        }
    }

    /// The wall-clock times one period offers, in order.
    ///
    /// An hourly period is its firing, the hour being what the period stepped,
    /// so it is returned whole. Every other frequency picks dates and crosses
    /// them with the hours and minutes the rule or the anchor gives.
    fn candidates(
        &self,
        period: NaiveDateTime,
        anchor: NaiveDateTime,
        hours: &[u32],
        minutes: &[u32],
    ) -> Vec<NaiveDateTime> {
        if self.frequency == Frequency::Hourly {
            return period.with_second(0).into_iter().collect();
        }
        let mut times: Vec<NaiveDateTime> = Vec::new();
        for day in self.days_of(period, anchor) {
            for hour in hours {
                for minute in minutes {
                    times.extend(day.and_hms_opt(*hour, *minute, 0));
                }
            }
        }
        times.sort_unstable();
        times
    }

    /// Which dates inside one period the rule selects, in order.
    fn days_of(&self, period: NaiveDateTime, anchor: NaiveDateTime) -> Vec<NaiveDate> {
        let date = period.date();
        match self.frequency {
            Frequency::Hourly | Frequency::Daily => vec![date],
            Frequency::Weekly => {
                let days = if self.by_day.is_empty() {
                    vec![anchor.weekday()]
                } else {
                    self.by_day.clone()
                };
                let weekday = i64::from(date.weekday().num_days_from_monday());
                let mut dates: Vec<NaiveDate> = days
                    .iter()
                    .filter_map(|day| {
                        date.checked_add_signed(TimeDelta::days(
                            i64::from(day.num_days_from_monday()) - weekday,
                        ))
                    })
                    .collect();
                dates.sort_unstable();
                dates
            }
            Frequency::Monthly => {
                let last = last_day(date.year(), date.month());
                let days = if self.by_month_day.is_empty() {
                    vec![anchor.day().min(last)]
                } else {
                    self.by_month_day.clone()
                };
                let mut dates: Vec<NaiveDate> = days
                    .iter()
                    .filter_map(|day| NaiveDate::from_ymd_opt(date.year(), date.month(), *day))
                    .collect();
                dates.sort_unstable();
                dates
            }
        }
    }
}

/// Adds whole months, clamping a day the target month does not have: 31
/// January plus one month is the last day of February, never 3 March.
fn months(from: NaiveDateTime, count: i64) -> Option<NaiveDateTime> {
    let zero = i64::from(from.year()) * MONTHS_PER_YEAR + i64::from(from.month0()) + count;
    let year = i32::try_from(zero.div_euclid(MONTHS_PER_YEAR)).ok()?;
    let month = u32::try_from(zero.rem_euclid(MONTHS_PER_YEAR) + 1).ok()?;
    let last = last_day(year, month);
    NaiveDate::from_ymd_opt(year, month, from.day().min(last))?.and_hms_opt(
        from.hour(),
        from.minute(),
        0,
    )
}

fn last_day(year: i32, month: u32) -> u32 {
    (SHORTEST_MONTH_DAYS..=LAST_MONTH_DAY)
        .rev()
        .find(|day| NaiveDate::from_ymd_opt(year, month, *day).is_some())
        .unwrap_or(SHORTEST_MONTH_DAYS)
}

fn list<T, F>(value: &str, read: F, clause: Clause) -> Result<Vec<T>>
where
    F: Fn(&str) -> Option<T>,
    T: Ord,
{
    let mut parsed = Vec::new();
    for raw in value
        .split(',')
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        parsed.push(read(raw).ok_or_else(|| Error::Value {
            clause,
            value: raw.to_string(),
        })?);
    }
    if parsed.is_empty() {
        return Err(Error::Blank { clause });
    }
    parsed.sort_unstable();
    parsed.dedup();
    Ok(parsed)
}

fn bounded(raw: &str, low: u32, high: u32) -> Option<u32> {
    raw.parse::<u32>()
        .ok()
        .filter(|parsed| (low..=high).contains(parsed))
}

fn positive(value: &str) -> Option<u32> {
    value.parse::<u32>().ok().filter(|parsed| *parsed >= 1)
}

fn timestamp(value: &str) -> Result<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(value, UNTIL_FORMAT)
        .map(|parsed| parsed.and_utc())
        .map_err(|_| Error::Until {
            value: value.to_string(),
        })
}
