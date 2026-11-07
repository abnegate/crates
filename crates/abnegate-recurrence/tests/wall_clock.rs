use abnegate_recurrence::Recurrence;
use abnegate_recurrence::Timezone;
use chrono::DateTime;
use chrono::Utc;

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("the fixture is a timestamp")
        .with_timezone(&Utc)
}

fn timezone(name: &str) -> Timezone {
    Timezone::parse(name).expect("the fixture is an IANA timezone")
}

fn firings(rule: &str, anchor: &str, timezone_name: &str, count: u32) -> Vec<DateTime<Utc>> {
    let rule = Recurrence::parse(rule).expect("the rule is accepted");
    let anchor = at(anchor);
    let mut fired = Vec::new();
    let mut after = anchor;
    for index in 1..=count {
        let next = rule
            .next_after(anchor, after, index, timezone(timezone_name))
            .expect("the rule keeps firing");
        fired.push(next);
        after = next;
    }
    fired
}

#[test]
fn a_daily_nine_written_at_plus_one_repeats_at_nine_there_and_not_an_hour_later() {
    assert_eq!(
        firings(
            "FREQ=DAILY;BYHOUR=9",
            "2026-11-10T09:00:00+01:00",
            "Europe/Berlin",
            2
        ),
        [
            at("2026-11-11T09:00:00+01:00"),
            at("2026-11-12T09:00:00+01:00")
        ],
        "read in UTC, BYHOUR=9 fires again at 10:00 the same morning and every day after"
    );
}

#[test]
fn a_monday_eight_at_plus_twelve_repeats_on_monday_morning_there() {
    for rule in ["FREQ=WEEKLY;BYDAY=MO;BYHOUR=8", "FREQ=WEEKLY;BYDAY=MO"] {
        assert_eq!(
            firings(rule, "2027-07-05T08:00:00+12:00", "Pacific/Auckland", 2),
            [
                at("2027-07-12T08:00:00+12:00"),
                at("2027-07-19T08:00:00+12:00")
            ],
            "{rule}: read in UTC this fires on Monday evening or Tuesday morning instead"
        );
    }
}

#[test]
fn a_time_with_no_hour_keeps_its_wall_clock_across_daylight_saving() {
    assert_eq!(
        firings(
            "FREQ=DAILY",
            "2027-03-27T09:00:00+00:00",
            "Europe/London",
            2
        ),
        [
            at("2027-03-28T09:00:00+01:00"),
            at("2027-03-29T09:00:00+01:00")
        ],
        "London springs forward on 28 March, and nine stays nine"
    );
    assert_eq!(
        firings(
            "FREQ=DAILY",
            "2027-10-30T09:00:00+01:00",
            "Europe/London",
            2
        ),
        [
            at("2027-10-31T09:00:00+00:00"),
            at("2027-11-01T09:00:00+00:00")
        ],
        "London falls back on 31 October"
    );
    assert_eq!(
        firings(
            "FREQ=WEEKLY",
            "2027-09-20T09:00:00+12:00",
            "Pacific/Auckland",
            1
        ),
        [at("2027-09-27T09:00:00+13:00")],
        "Auckland springs forward on 26 September"
    );
}

#[test]
fn a_time_london_skips_fires_when_the_clocks_change_and_a_repeated_one_fires_once() {
    assert_eq!(
        firings(
            "FREQ=DAILY;BYHOUR=1;BYMINUTE=30",
            "2027-03-27T01:30:00+00:00",
            "Europe/London",
            2
        ),
        [
            at("2027-03-28T02:00:00+01:00"),
            at("2027-03-29T01:30:00+01:00")
        ],
        "01:30 does not exist on 28 March, so it fires at 02:00 BST, the instant the clocks change"
    );
    assert_eq!(
        firings(
            "FREQ=DAILY;BYHOUR=1;BYMINUTE=30",
            "2027-10-30T01:30:00+01:00",
            "Europe/London",
            2
        ),
        [
            at("2027-10-31T01:30:00+01:00"),
            at("2027-11-01T01:30:00+00:00")
        ],
        "01:30 happens twice on 31 October, and only the first, in BST, fires"
    );
}

#[test]
fn a_time_auckland_skips_fires_when_the_clocks_change_and_a_repeated_one_fires_once() {
    assert_eq!(
        firings(
            "FREQ=DAILY;BYHOUR=2;BYMINUTE=30",
            "2027-09-25T02:30:00+12:00",
            "Pacific/Auckland",
            2
        ),
        [
            at("2027-09-26T03:00:00+13:00"),
            at("2027-09-27T02:30:00+13:00")
        ],
        "02:30 does not exist on 26 September, so it fires at 03:00 NZDT"
    );
    assert_eq!(
        firings(
            "FREQ=DAILY;BYHOUR=2;BYMINUTE=30",
            "2027-04-03T02:30:00+13:00",
            "Pacific/Auckland",
            2
        ),
        [
            at("2027-04-04T02:30:00+13:00"),
            at("2027-04-05T02:30:00+12:00")
        ],
        "02:30 happens twice on 4 April, and only the first, in NZDT, fires"
    );
}

#[test]
fn an_hourly_rule_counts_elapsed_hours_through_a_repeated_hour() {
    assert_eq!(
        firings(
            "FREQ=HOURLY",
            "2027-10-31T00:00:00+01:00",
            "Europe/London",
            3
        ),
        [
            at("2027-10-31T01:00:00+01:00"),
            at("2027-10-31T01:00:00+00:00"),
            at("2027-10-31T02:00:00+00:00"),
        ]
    );
}

#[test]
fn a_monthly_first_at_plus_twelve_stays_on_the_first_there() {
    assert_eq!(
        firings(
            "FREQ=MONTHLY;BYMONTHDAY=1;BYHOUR=8",
            "2027-07-01T08:00:00+12:00",
            "Pacific/Auckland",
            2
        ),
        [
            at("2027-08-01T08:00:00+12:00"),
            at("2027-09-01T08:00:00+12:00")
        ],
        "read in UTC, the 1st at 08:00 +12:00 is the last day of the month before"
    );
}

#[test]
fn until_is_an_instant_however_the_timezone_reads_it() {
    let rule = Recurrence::parse("FREQ=DAILY;BYHOUR=9;UNTIL=20270706T210000Z").unwrap();
    let anchor = at("2027-07-05T09:00:00+12:00");
    let auckland = timezone("Pacific/Auckland");
    let second = rule.next_after(anchor, anchor, 1, auckland);
    assert_eq!(second, Some(at("2027-07-06T09:00:00+12:00")));
    let third = rule.next_after(anchor, second.unwrap(), 2, auckland);
    assert_eq!(
        third,
        Some(at("2027-07-07T09:00:00+12:00")),
        "UNTIL includes the instant it names"
    );
    assert_eq!(rule.next_after(anchor, third.unwrap(), 3, auckland), None);
}
