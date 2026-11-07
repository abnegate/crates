# abnegate-recurrence

An RFC 5545 `RRULE` subset evaluated on an IANA timezone's wall clock.
`Recurrence` reads a rule and answers with the next instant it fires after a
given moment, reading every clause on a `Timezone`'s clock so nine o'clock
stays nine o'clock across a daylight-saving change. It is pure: a rule and a
clock reading in, an instant out, with no store and no worker.

## The subset

`FREQ` (`HOURLY`, `DAILY`, `WEEKLY` or `MONTHLY`), `INTERVAL`, `BYDAY`,
`BYHOUR`, `BYMINUTE`, `BYMONTHDAY`, `UNTIL` (as `20260914T090000Z`) and
`COUNT`, with or without the `RRULE:` prefix. Names and values are read in any
case.

A clause outside the subset is refused, never ignored, and so is a clause the
rule's frequency never reads: `BYDAY` needs `WEEKLY`, `BYMONTHDAY` needs
`MONTHLY`, and `BYHOUR` and `BYMINUTE` need `DAILY`, `WEEKLY` or `MONTHLY`. A
rule that silently dropped one would fire at a time nobody asked for. Every
refusal is an `Error` whose text names the clause and what it wanted.

## The wall clock

- A time the clocks skip fires at the instant they change: 01:30 on the morning
  London springs forward fires at 02:00 BST.
- A time the clocks show twice fires at the first of the two, once: 01:30 on
  the morning London falls back fires at 01:30 BST and not an hour later.
- An hourly rule counts elapsed UTC hours, which no clock change moves.
- `BYMONTHDAY=31` skips the months without a 31st, as RFC 5545 does, while a
  day inherited from the anchor clamps to the month's last day, so a rule
  anchored on 31 January fires on 28 February.
- A rule nothing can satisfy, such as the 31st of every February, gives `None`
  after a bounded search rather than walking forward for ever.

`period()` is the shortest gap between two firings rather than the average one,
so a host that holds rules to a minimum cadence compares it against its own
floor. This crate applies none.

## Features

None.

## Usage

```sh
cargo add abnegate-recurrence chrono
```

```rust
use abnegate_recurrence::Recurrence;
use abnegate_recurrence::Timezone;
use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;

let rule = Recurrence::parse("RRULE:FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9").expect("a rule");
let london = Timezone::parse("Europe/London").expect("a timezone");
let instant = |text: &str| text.parse::<DateTime<Utc>>().expect("an instant");

let anchor = instant("2027-03-22T09:00:00Z");
let friday = rule.next_after(anchor, anchor, 1, london);
assert_eq!(friday, Some(instant("2027-03-26T09:00:00Z")));

let monday = rule.next_after(anchor, instant("2027-03-26T09:00:00Z"), 2, london);
assert_eq!(monday, Some(instant("2027-03-29T08:00:00Z")));

assert_eq!(rule.period(), TimeDelta::days(3));
```

`anchor` is the first firing, and supplies whatever the rule does not state:
`FREQ=WEEKLY` alone repeats on the anchor's weekday and time. `fired` is how
many firings have already happened, which `COUNT` is measured against.
`fires_at` says whether a rule anchored at an instant would fire there itself,
which is how a host checks that a first firing and its repeats agree.
