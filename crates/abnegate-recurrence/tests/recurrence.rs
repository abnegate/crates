use abnegate_recurrence::Recurrence;
use abnegate_recurrence::Timezone;
use chrono::DateTime;
use chrono::Datelike;
use chrono::NaiveDate;
use chrono::TimeDelta;
use chrono::Utc;
use chrono::Weekday;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const LONGER_THAN_ANY_JUMP: Duration = Duration::from_secs(10);

fn at_utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("the fixture is a timestamp")
        .with_timezone(&Utc)
}

fn refusal(rule: &str) -> String {
    Recurrence::parse(rule).expect_err(rule).to_string()
}

#[test]
fn the_prefix_is_optional_and_the_rule_is_the_same_either_way() {
    assert_eq!(
        Recurrence::parse("RRULE:FREQ=DAILY;BYHOUR=9"),
        Recurrence::parse("FREQ=DAILY;BYHOUR=9")
    );
}

#[test]
fn clause_names_and_values_are_read_in_any_case() {
    assert_eq!(
        Recurrence::parse("freq=Weekly;byday=mo, Fr;ByHour=9"),
        Recurrence::parse("FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9")
    );
}

#[test]
fn a_clause_this_build_does_not_implement_is_refused_and_not_ignored() {
    let refused = refusal("FREQ=MONTHLY;BYSETPOS=-1;BYDAY=FR");
    assert!(refused.contains("BYSETPOS"), "{refused}");
    assert!(
        refused.contains("FREQ"),
        "the refusal lists what is accepted: {refused}"
    );
}

#[test]
fn a_rule_with_no_frequency_or_a_bad_one_is_refused_by_name() {
    for (rule, expected) in [
        ("INTERVAL=2", "FREQ"),
        ("FREQ=SECONDLY", "HOURLY"),
        ("FREQ=DAILY;INTERVAL=0", "INTERVAL"),
        ("FREQ=DAILY;BYHOUR=24", "BYHOUR"),
        ("FREQ=DAILY;BYMINUTE=60", "BYMINUTE"),
        ("FREQ=WEEKLY;BYDAY=FUNDAY", "BYDAY"),
        ("FREQ=DAILY;UNTIL=tomorrow", "UNTIL"),
        ("", "FREQ"),
    ] {
        let refused = refusal(rule);
        assert!(refused.contains(expected), "{rule}: {refused}");
    }
}

#[test]
fn a_rule_faster_than_an_hour_parses_and_reports_its_shortest_gap() {
    for (rule, gap) in [
        (
            "FREQ=DAILY;BYHOUR=0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23;BYMINUTE=0,30",
            30,
        ),
        (
            "FREQ=WEEKLY;BYHOUR=0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23;BYMINUTE=0,5,10,15,20,25,30,35",
            5,
        ),
        ("FREQ=DAILY;BYHOUR=0,1;BYMINUTE=0,30", 30),
        ("FREQ=DAILY;BYMINUTE=0,30", 30),
        ("FREQ=WEEKLY;BYDAY=MO,TU;BYHOUR=0,23;BYMINUTE=0,45", 15),
        ("FREQ=MONTHLY;BYMONTHDAY=1,2;BYHOUR=0,23;BYMINUTE=0,45", 15),
    ] {
        let period = Recurrence::parse(rule).expect(rule).period();
        assert_eq!(
            period.num_minutes(),
            gap,
            "{rule} should report its own shortest gap"
        );
    }

    for rule in [
        "FREQ=HOURLY",
        "FREQ=HOURLY;INTERVAL=4",
        "FREQ=DAILY;BYHOUR=9,17",
        "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9",
        "FREQ=DAILY;BYHOUR=0,23",
        "FREQ=WEEKLY;BYDAY=MO,TU;BYHOUR=0,23",
        "FREQ=MONTHLY;BYMONTHDAY=1,2;BYHOUR=0,23",
    ] {
        let period = Recurrence::parse(rule).expect(rule).period();
        assert!(
            period.num_minutes() >= 60,
            "{rule} is at or under once an hour: {period}"
        );
    }
}

#[test]
fn a_period_is_the_shortest_gap_between_firings_not_the_average_one() {
    for (rule, minutes) in [
        ("FREQ=HOURLY", 60),
        ("FREQ=HOURLY;INTERVAL=6", 360),
        ("FREQ=DAILY", 24 * 60),
        ("FREQ=DAILY;INTERVAL=3", 3 * 24 * 60),
        ("FREQ=DAILY;BYHOUR=9,17", 8 * 60),
        ("FREQ=DAILY;BYHOUR=0,23", 60),
        ("FREQ=WEEKLY", 7 * 24 * 60),
        ("FREQ=WEEKLY;BYDAY=MO,TU", 24 * 60),
        ("FREQ=WEEKLY;BYDAY=MO,FR", 3 * 24 * 60),
        ("FREQ=WEEKLY;BYDAY=MO,TU;BYHOUR=0,23", 60),
        ("FREQ=MONTHLY", 28 * 24 * 60),
        ("FREQ=MONTHLY;BYMONTHDAY=1,15", 14 * 24 * 60),
        ("FREQ=MONTHLY;BYMONTHDAY=1,29", 24 * 60),
    ] {
        let period = Recurrence::parse(rule).expect(rule).period();
        assert_eq!(period.num_minutes(), minutes, "{rule}");
    }
}

#[test]
fn a_clause_the_frequency_never_reads_is_refused_rather_than_ignored() {
    for (rule, clause) in [
        ("FREQ=HOURLY;BYHOUR=9", "BYHOUR"),
        ("FREQ=HOURLY;BYMINUTE=0,30", "BYMINUTE"),
        ("FREQ=HOURLY;BYDAY=MO", "BYDAY"),
        ("FREQ=DAILY;BYDAY=MO,FR", "BYDAY"),
        ("FREQ=DAILY;BYMONTHDAY=1", "BYMONTHDAY"),
        ("FREQ=WEEKLY;BYMONTHDAY=1", "BYMONTHDAY"),
        ("FREQ=MONTHLY;BYDAY=FR", "BYDAY"),
    ] {
        let refused = refusal(rule);
        assert!(refused.contains(clause), "{rule}: {refused}");
        assert!(
            refused.contains("does not use"),
            "the refusal must say the clause is unread, not that it is unknown: {refused}"
        );
    }
}

#[test]
fn every_clause_a_frequency_does_read_is_still_accepted() {
    for rule in [
        "FREQ=HOURLY",
        "FREQ=HOURLY;INTERVAL=6",
        "FREQ=DAILY;BYHOUR=9,17",
        "FREQ=DAILY;BYHOUR=9;BYMINUTE=30",
        "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9",
        "FREQ=MONTHLY;BYMONTHDAY=1;BYHOUR=8;BYMINUTE=5",
    ] {
        assert!(
            Recurrence::parse(rule).is_ok(),
            "{rule} must still be accepted"
        );
    }
}

#[test]
fn until_and_count_cannot_both_stop_the_same_rule() {
    let refused = refusal("FREQ=DAILY;UNTIL=20260101T000000Z;COUNT=3");
    assert!(refused.contains("not both"), "{refused}");
}

#[test]
fn a_rule_that_names_no_time_repeats_at_the_anchors_own() {
    let anchor = at_utc("2026-09-14T06:47:00Z");
    let rule = Recurrence::parse("FREQ=DAILY").expect("accepted");

    assert_eq!(
        rule.next_after(anchor, anchor, 1, Timezone::UTC),
        Some(at_utc("2026-09-15T06:47:00Z"))
    );
}

#[test]
fn a_daily_rule_walks_its_hours_in_order_and_then_the_next_day() {
    let anchor = at_utc("2026-09-14T09:00:00Z");
    let rule = Recurrence::parse("FREQ=DAILY;BYHOUR=9,17;BYMINUTE=3").expect("accepted");

    let first = rule
        .next_after(anchor, at_utc("2026-09-14T00:00:00Z"), 0, Timezone::UTC)
        .expect("a firing");
    assert_eq!(first, at_utc("2026-09-14T09:03:00Z"));
    assert_eq!(
        rule.next_after(anchor, first, 1, Timezone::UTC),
        Some(at_utc("2026-09-14T17:03:00Z"))
    );
    assert_eq!(
        rule.next_after(anchor, at_utc("2026-09-14T17:03:00Z"), 2, Timezone::UTC),
        Some(at_utc("2026-09-15T09:03:00Z"))
    );
}

#[test]
fn a_weekly_rule_fires_on_each_named_day_and_then_skips_its_interval() {
    let anchor = at_utc("2026-09-14T09:00:00Z");
    assert_eq!(anchor.weekday(), Weekday::Mon);
    let rule = Recurrence::parse("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TH;BYHOUR=9").expect("accepted");

    let thursday = rule
        .next_after(anchor, anchor, 1, Timezone::UTC)
        .expect("a firing");
    assert_eq!(thursday, at_utc("2026-09-17T09:00:00Z"));
    assert_eq!(thursday.weekday(), Weekday::Thu);

    assert_eq!(
        rule.next_after(anchor, thursday, 2, Timezone::UTC),
        Some(at_utc("2026-09-28T09:00:00Z")),
        "two weeks on from the anchor's week, not one"
    );
}

#[test]
fn a_monthly_rule_clamps_a_day_the_month_does_not_have() {
    let anchor = at_utc("2026-01-31T08:00:00Z");
    let rule = Recurrence::parse("FREQ=MONTHLY").expect("accepted");

    let february = rule
        .next_after(anchor, anchor, 1, Timezone::UTC)
        .expect("a firing");
    assert_eq!(february, at_utc("2026-02-28T08:00:00Z"));
    assert_eq!(
        rule.next_after(anchor, february, 2, Timezone::UTC),
        Some(at_utc("2026-03-31T08:00:00Z")),
        "a clamped month must not drag the following ones back with it"
    );
}

#[test]
fn a_monthly_rule_on_a_day_no_month_has_gives_up_rather_than_looping() {
    let anchor = at_utc("2026-01-01T08:00:00Z");
    let rule = Recurrence::parse("FREQ=MONTHLY;BYMONTHDAY=31;BYHOUR=8").expect("accepted");

    let march = rule.next_after(anchor, at_utc("2026-02-01T00:00:00Z"), 1, Timezone::UTC);
    assert_eq!(
        march,
        Some(at_utc("2026-03-31T08:00:00Z")),
        "only the long months, in order, and never a 31st that does not exist"
    );
}

#[test]
fn an_hourly_rule_counts_from_the_anchor_and_keeps_its_minute() {
    let anchor = at_utc("2026-09-14T06:47:00Z");
    let rule = Recurrence::parse("FREQ=HOURLY;INTERVAL=6").expect("accepted");

    let next = rule
        .next_after(anchor, anchor, 1, Timezone::UTC)
        .expect("a firing");
    assert_eq!(next, at_utc("2026-09-14T12:47:00Z"));
    assert_eq!(
        rule.next_after(anchor, next, 2, Timezone::UTC),
        Some(at_utc("2026-09-14T18:47:00Z"))
    );
}

#[test]
fn count_stops_the_rule_after_the_firings_it_names() {
    let anchor = at_utc("2026-09-14T09:00:00Z");
    let rule = Recurrence::parse("FREQ=DAILY;COUNT=3").expect("accepted");

    assert!(rule.next_after(anchor, anchor, 1, Timezone::UTC).is_some());
    assert!(rule.next_after(anchor, anchor, 2, Timezone::UTC).is_some());
    assert_eq!(
        rule.next_after(anchor, anchor, 3, Timezone::UTC),
        None,
        "the third firing is the last, so a fourth is never scheduled"
    );
}

#[test]
fn until_stops_the_rule_at_the_instant_it_names() {
    let anchor = at_utc("2026-09-14T09:00:00Z");
    let rule = Recurrence::parse("FREQ=DAILY;UNTIL=20260916T090000Z").expect("accepted");

    assert_eq!(
        rule.next_after(anchor, anchor, 1, Timezone::UTC),
        Some(at_utc("2026-09-15T09:00:00Z"))
    );
    assert_eq!(
        rule.next_after(anchor, at_utc("2026-09-15T09:00:00Z"), 2, Timezone::UTC),
        Some(at_utc("2026-09-16T09:00:00Z")),
        "UNTIL includes the instant it names"
    );
    assert_eq!(
        rule.next_after(anchor, at_utc("2026-09-16T09:00:00Z"), 3, Timezone::UTC),
        None
    );
}

#[test]
fn a_schedule_resumed_after_an_outage_fires_next_rather_than_catching_up() {
    let anchor = at_utc("2026-09-01T09:00:00Z");
    let rule = Recurrence::parse("FREQ=DAILY;BYHOUR=9").expect("accepted");

    assert_eq!(
        rule.next_after(anchor, at_utc("2026-09-14T06:47:00Z"), 13, Timezone::UTC),
        Some(at_utc("2026-09-14T09:00:00Z"))
    );
}

#[test]
fn a_rule_nothing_can_satisfy_gives_up_instead_of_spinning() {
    let february = at_utc("2026-02-01T09:00:00Z");
    let rule =
        Recurrence::parse("FREQ=MONTHLY;INTERVAL=12;BYMONTHDAY=31;BYHOUR=9").expect("accepted");

    assert_eq!(rule.next_after(february, february, 1, Timezone::UTC), None);
}

#[test]
fn a_first_firing_is_a_time_the_schedule_itself_would_fire_at() {
    let london = Timezone::parse("Europe/London").expect("a timezone");
    for (rule, due, fires) in [
        (
            "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9",
            "2027-07-05T09:00:00+01:00",
            true,
        ),
        (
            "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9",
            "2027-07-06T09:00:00+01:00",
            false,
        ),
        ("FREQ=WEEKLY", "2027-07-06T09:00:00+01:00", true),
        (
            "FREQ=MONTHLY;BYMONTHDAY=1;BYHOUR=8",
            "2027-07-01T08:00:00+01:00",
            true,
        ),
        (
            "FREQ=MONTHLY;BYMONTHDAY=1;BYHOUR=8",
            "2027-07-02T08:00:00+01:00",
            false,
        ),
        ("FREQ=DAILY;BYHOUR=9", "2027-07-05T09:00:00Z", false),
        ("FREQ=DAILY;BYHOUR=9", "2027-07-05T09:00:30+01:00", true),
        ("FREQ=HOURLY", "2027-07-05T09:17:42.123456+01:00", true),
        (
            "FREQ=DAILY;BYHOUR=1;BYMINUTE=30",
            "2027-03-28T02:00:00+01:00",
            true,
        ),
        (
            "FREQ=DAILY;BYHOUR=1;BYMINUTE=30",
            "2027-10-31T01:30:00+01:00",
            true,
        ),
        (
            "FREQ=DAILY;BYHOUR=1;BYMINUTE=30",
            "2027-10-31T01:30:00+00:00",
            false,
        ),
    ] {
        let parsed = Recurrence::parse(rule).expect(rule);
        assert_eq!(
            parsed.fires_at(at_utc(due), london),
            fires,
            "{rule} at {due}"
        );
    }
}

#[test]
fn a_monthly_interval_past_the_calendar_gives_up_instead_of_overflowing() {
    let anchor = at_utc("2027-01-15T09:00:00Z");
    for rule in [
        "FREQ=MONTHLY;INTERVAL=2147483647",
        "FREQ=MONTHLY;INTERVAL=4294967295",
    ] {
        let parsed = Recurrence::parse(rule).expect(rule);
        assert_eq!(
            parsed.next_after(anchor, anchor, 1, Timezone::UTC),
            None,
            "{rule} has no second period inside the calendar"
        );
        assert!(
            parsed.fires_at(anchor, Timezone::UTC),
            "{rule} still fires at its anchor"
        );
    }
}

#[test]
fn a_weekly_period_at_the_end_of_the_calendar_skips_the_days_past_it() {
    let monday = at_utc("2027-01-04T09:00:00Z");
    let sunday = at_utc("2027-01-10T09:00:00Z");
    let weeks = (NaiveDate::MAX - monday.date_naive()).num_weeks();
    let last = monday + TimeDelta::weeks(weeks);
    assert_eq!(
        last.date_naive().checked_add_signed(TimeDelta::days(6)),
        None,
        "the last period's Sunday lies past the calendar"
    );
    let rule =
        Recurrence::parse(&format!("FREQ=WEEKLY;INTERVAL={weeks};BYDAY=MO,SU")).expect("accepted");

    assert_eq!(
        rule.next_after(monday, sunday, 2, Timezone::UTC),
        Some(last)
    );
    assert_eq!(rule.next_after(monday, last, 3, Timezone::UTC), None);
}

#[test]
fn a_weekly_period_at_the_start_of_the_calendar_keeps_the_days_inside_it() {
    for day in 0..7 {
        let anchor = at_utc("2027-01-04T09:00:00Z") + TimeDelta::days(day);
        let weeks = (anchor.date_naive() - NaiveDate::MIN).num_weeks();
        for interval in [weeks - 1, weeks, weeks + 1] {
            let rule = format!("FREQ=WEEKLY;INTERVAL={interval};BYDAY=MO,TU,WE,TH,FR,SA,SU");
            let parsed = Recurrence::parse(&rule).expect(&rule);
            assert!(
                parsed.fires_at(anchor, Timezone::UTC),
                "{rule} fires at its anchor {anchor}"
            );
        }
    }
}

#[test]
fn the_prefix_is_matched_in_upper_case_exactly_as_written() {
    assert_eq!(
        refusal("rrule:FREQ=DAILY"),
        "RRULE:FREQ is not a clause this build accepts. Use FREQ, INTERVAL, BYDAY, BYHOUR, \
         BYMINUTE, BYMONTHDAY, UNTIL or COUNT."
    );
}

#[test]
fn until_is_read_only_in_its_upper_case_utc_form() {
    assert_eq!(
        refusal("FREQ=DAILY;until=20260914t090000z"),
        "UNTIL=20260914t090000z must read as 20260914T090000Z."
    );
    assert!(Recurrence::parse("FREQ=DAILY;until=20260914T090000Z").is_ok());
}

#[test]
fn an_after_before_the_anchor_reads_the_periods_before_it_in_its_phase() {
    let anchor = at_utc("2026-09-14T09:00:00Z");
    let daily = Recurrence::parse("FREQ=DAILY").expect("accepted");
    assert_eq!(
        daily.next_after(anchor, at_utc("2026-09-01T12:00:00Z"), 1, Timezone::UTC),
        Some(at_utc("2026-09-02T09:00:00Z"))
    );

    let alternate = Recurrence::parse("FREQ=DAILY;INTERVAL=2").expect("accepted");
    assert_eq!(
        alternate.next_after(anchor, at_utc("2026-09-02T12:00:00Z"), 1, Timezone::UTC),
        Some(at_utc("2026-09-04T09:00:00Z")),
        "every other day counted back from the anchor, so 4 September and not 3"
    );

    assert!(daily.fires_at(anchor, Timezone::UTC));
    assert_eq!(
        daily.next_after(anchor, anchor, 1, Timezone::UTC),
        Some(at_utc("2026-09-15T09:00:00Z")),
        "asked at the anchor, the next firing is the one after it"
    );
}

#[test]
fn a_period_is_measured_on_the_wall_clock_not_in_elapsed_time() {
    let london = Timezone::parse("Europe/London").expect("a timezone");
    let rule = Recurrence::parse("FREQ=DAILY;BYHOUR=0,2").expect("accepted");
    assert_eq!(rule.period(), TimeDelta::hours(2));

    let anchor = at_utc("2027-03-27T00:00:00Z");
    let midnight = rule
        .next_after(anchor, at_utc("2027-03-27T23:00:00Z"), 1, london)
        .expect("a firing");
    let two = rule
        .next_after(anchor, midnight, 2, london)
        .expect("a firing");
    assert_eq!(midnight, at_utc("2027-03-28T00:00:00Z"));
    assert_eq!(two, at_utc("2027-03-28T01:00:00Z"));
    assert_eq!(
        two - midnight,
        TimeDelta::hours(1),
        "London springs forward between the two"
    );
}

#[test]
fn an_interval_past_the_largest_i32_counts_in_full() {
    for (rule, period) in [
        (
            "FREQ=HOURLY;INTERVAL=2147483648",
            TimeDelta::hours(2_147_483_648),
        ),
        (
            "FREQ=HOURLY;INTERVAL=4294967295",
            TimeDelta::hours(4_294_967_295),
        ),
        (
            "FREQ=DAILY;INTERVAL=4294967295",
            TimeDelta::days(4_294_967_295),
        ),
        (
            "FREQ=WEEKLY;INTERVAL=4294967295",
            TimeDelta::weeks(4_294_967_295),
        ),
        (
            "FREQ=MONTHLY;INTERVAL=2147483648",
            TimeDelta::days(28 * 2_147_483_648),
        ),
        ("FREQ=MONTHLY;INTERVAL=4294967295", TimeDelta::MAX),
    ] {
        assert_eq!(
            Recurrence::parse(rule).expect(rule).period(),
            period,
            "{rule}"
        );
    }

    let anchor = at_utc("2027-01-15T09:00:00Z");
    let hourly = Recurrence::parse("FREQ=HOURLY;INTERVAL=2147483648").expect("accepted");
    let next = hourly
        .next_after(anchor, anchor, 1, Timezone::UTC)
        .expect("2^31 hours on still lies inside the calendar");
    assert_eq!(next, anchor + TimeDelta::hours(2_147_483_648));
    assert!(hourly.fires_at(anchor, Timezone::UTC));
    assert!(hourly.fires_at(next, Timezone::UTC));

    let longest = Recurrence::parse("FREQ=HOURLY;INTERVAL=4294967295").expect("accepted");
    assert_eq!(
        longest.next_after(anchor, anchor, 1, Timezone::UTC),
        None,
        "2^32 - 1 hours on lies past the end of the calendar"
    );
}

#[test]
fn a_monthly_rule_keeps_the_31st_of_the_calendars_last_december() {
    let last_year = NaiveDate::MAX.year();
    let december = NaiveDate::from_ymd_opt(last_year, 12, 31)
        .and_then(|date| date.and_hms_opt(9, 0, 0))
        .expect("the calendar ends on 31 December")
        .and_utc();
    let january = NaiveDate::from_ymd_opt(last_year, 1, 31)
        .and_then(|date| date.and_hms_opt(9, 0, 0))
        .expect("a date in the calendar's last year")
        .and_utc();
    let november = NaiveDate::from_ymd_opt(last_year, 11, 30)
        .and_then(|date| date.and_hms_opt(9, 0, 0))
        .expect("a date in the calendar's last year")
        .and_utc();
    let rule = Recurrence::parse("FREQ=MONTHLY").expect("accepted");

    assert!(rule.fires_at(december, Timezone::UTC));
    assert_eq!(
        rule.next_after(january, november, 11, Timezone::UTC),
        Some(december),
        "the 31st, not the 28th"
    );
    assert_eq!(rule.next_after(january, december, 12, Timezone::UTC), None);
}

fn london_firing(rule: &str, anchor: &str, after: &str) -> Option<DateTime<Utc>> {
    let london = Timezone::parse("Europe/London").expect("a timezone");
    let counted = Recurrence::parse(&format!("{rule};COUNT=2")).expect("accepted");
    assert!(
        counted.fires_at(at_utc(anchor), london),
        "{anchor} is a firing"
    );
    assert_eq!(
        counted.next_after(at_utc(anchor), at_utc(after), 2, london),
        None,
        "{rule} has spent COUNT=2, however many periods lie between {anchor} and {after}"
    );
    counted.next_after(at_utc(anchor), at_utc(after), 1, london)
}

#[test]
fn a_thirty_year_old_anchor_keeps_its_phase_across_daylight_saving() {
    for (rule, anchor, after, next) in [
        (
            "FREQ=HOURLY",
            "1997-03-28T01:00:00Z",
            "2027-03-28T01:00:00Z",
            "2027-03-28T02:00:00Z",
        ),
        (
            "FREQ=HOURLY;INTERVAL=7",
            "1997-03-30T00:17:42.5Z",
            "2027-03-28T01:00:00Z",
            "2027-03-28T07:17:00.5Z",
        ),
        (
            "FREQ=DAILY",
            "1997-03-28T09:00:00Z",
            "2027-03-28T08:00:00Z",
            "2027-03-29T08:00:00Z",
        ),
        (
            "FREQ=DAILY;INTERVAL=3;BYHOUR=1;BYMINUTE=30",
            "1997-10-25T00:30:00Z",
            "2027-03-27T12:00:00Z",
            "2027-03-28T01:00:00Z",
        ),
        (
            "FREQ=DAILY;INTERVAL=3;BYHOUR=1;BYMINUTE=30",
            "1997-10-26T00:30:00Z",
            "2027-03-27T12:00:00Z",
            "2027-03-29T00:30:00Z",
        ),
        (
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=SU;BYHOUR=1;BYMINUTE=30",
            "1997-03-23T01:30:00Z",
            "2027-03-21T12:00:00Z",
            "2027-03-28T01:00:00Z",
        ),
        (
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=SU;BYHOUR=1;BYMINUTE=30",
            "1997-03-30T01:00:00Z",
            "2027-03-21T12:00:00Z",
            "2027-04-04T00:30:00Z",
        ),
        (
            "FREQ=MONTHLY",
            "1997-01-31T09:00:00Z",
            "2027-02-15T00:00:00Z",
            "2027-02-28T09:00:00Z",
        ),
        (
            "FREQ=MONTHLY;INTERVAL=7;BYMONTHDAY=31",
            "1997-01-31T09:00:00Z",
            "2027-01-01T00:00:00Z",
            "2027-05-31T08:00:00Z",
        ),
    ] {
        assert_eq!(
            london_firing(rule, anchor, after),
            Some(at_utc(next)),
            "{rule} anchored at {anchor}, asked after {after}"
        );
    }
}

#[test]
fn an_anchor_two_years_ahead_counts_its_phase_back_across_daylight_saving() {
    for (rule, anchor, after, next) in [
        (
            "FREQ=HOURLY;INTERVAL=7",
            "2029-03-28T00:17:42.5Z",
            "2027-03-28T01:00:00Z",
            "2027-03-28T02:17:00.5Z",
        ),
        (
            "FREQ=DAILY;INTERVAL=3;BYHOUR=1;BYMINUTE=30",
            "2029-03-24T01:30:00Z",
            "2027-03-27T12:00:00Z",
            "2027-03-29T00:30:00Z",
        ),
        (
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=SU;BYHOUR=1;BYMINUTE=30",
            "2029-04-01T00:30:00Z",
            "2027-03-21T12:00:00Z",
            "2027-04-04T00:30:00Z",
        ),
        (
            "FREQ=MONTHLY",
            "2029-01-31T09:00:00Z",
            "2027-02-15T00:00:00Z",
            "2027-02-28T09:00:00Z",
        ),
    ] {
        assert_eq!(
            london_firing(rule, anchor, after),
            Some(at_utc(next)),
            "{rule} anchored at {anchor}, asked after {after}"
        );
    }
}

#[test]
fn a_leap_second_anchor_answers_as_a_walk_from_it_does() {
    for (rule, anchor, after, next) in [
        (
            "FREQ=HOURLY",
            "2016-12-31T23:59:60.5Z",
            "2017-01-01T00:00:00.2Z",
            "2017-01-01T00:59:00.5Z",
        ),
        (
            "FREQ=DAILY;INTERVAL=3",
            "2016-12-31T23:59:60.5Z",
            "2017-01-01T00:00:00.2Z",
            "2017-01-03T23:59:00Z",
        ),
        (
            "FREQ=WEEKLY;INTERVAL=2",
            "1995-12-31T23:59:60.5Z",
            "1996-01-01T00:00:00.2Z",
            "1996-01-14T23:59:00Z",
        ),
    ] {
        let parsed = Recurrence::parse(rule).expect("accepted");
        assert_eq!(
            parsed.next_after(at_utc(anchor), at_utc(after), 0, Timezone::UTC),
            Some(at_utc(next)),
            "{rule} anchored at {anchor}, asked after {after}"
        );
    }
}

fn calendar_minute(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(hour, minute, 0))
        .expect("a minute the calendar holds")
        .and_utc()
}

fn answered_without_walking(
    rule: &str,
    anchor: DateTime<Utc>,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let recurrence = Recurrence::parse(rule).expect("accepted");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || sender.send(recurrence.next_after(anchor, after, 0, Timezone::UTC)));
    receiver
        .recv_timeout(LONGER_THAN_ANY_JUMP)
        .unwrap_or_else(|error| panic!("{rule} from {anchor} after {after} walked: {error}"))
}

#[test]
fn the_whole_calendar_between_anchor_and_after_is_jumped_rather_than_walked() {
    let first = NaiveDate::MIN.year();
    let last = NaiveDate::MAX.year();
    let anchor = calendar_minute(first, 1, 1, 1, 17);
    let june = calendar_minute(last, 6, 15, 12, 30);
    for (rule, after, next) in [
        (
            "FREQ=HOURLY",
            june,
            Some(calendar_minute(last, 6, 15, 13, 17)),
        ),
        (
            "FREQ=HOURLY;INTERVAL=7",
            june,
            Some(calendar_minute(last, 6, 15, 19, 17)),
        ),
        (
            "FREQ=DAILY;INTERVAL=3",
            june,
            Some(calendar_minute(last, 6, 18, 1, 17)),
        ),
        (
            "FREQ=WEEKLY;INTERVAL=2",
            june,
            Some(calendar_minute(last, 6, 28, 1, 17)),
        ),
        ("FREQ=HOURLY", DateTime::<Utc>::MAX_UTC, None),
    ] {
        assert_eq!(
            answered_without_walking(rule, anchor, after),
            next,
            "{rule} after {after}"
        );
    }
}

#[test]
fn a_landing_before_the_calendar_starts_rises_to_the_lowest_period_it_can_hold() {
    let first = NaiveDate::MIN.year();
    let anchor = at_utc("2027-01-15T09:17:00Z");
    let earliest = DateTime::<Utc>::MIN_UTC;
    let two_hours_in = earliest + TimeDelta::hours(2);
    for (rule, after, next) in [
        ("FREQ=HOURLY", earliest, calendar_minute(first, 1, 1, 0, 17)),
        (
            "FREQ=HOURLY;INTERVAL=7",
            earliest,
            calendar_minute(first, 1, 1, 5, 17),
        ),
        (
            "FREQ=HOURLY;INTERVAL=7",
            two_hours_in,
            calendar_minute(first, 1, 1, 5, 17),
        ),
        (
            "FREQ=DAILY;INTERVAL=3",
            earliest,
            calendar_minute(first, 1, 3, 9, 17),
        ),
        (
            "FREQ=WEEKLY;INTERVAL=2",
            earliest,
            calendar_minute(first, 1, 2, 9, 17),
        ),
    ] {
        assert_eq!(
            answered_without_walking(rule, anchor, after),
            Some(next),
            "{rule} after {after}"
        );
    }
}

#[test]
fn a_rule_that_walks_reaches_a_period_further_off_than_a_search_looks() {
    for (rule, anchor, after, next) in [
        (
            "FREQ=MONTHLY",
            "1800-01-31T09:00:00Z",
            "2027-02-15T00:00:00Z",
            "2027-02-28T09:00:00Z",
        ),
        (
            "FREQ=DAILY",
            "1900-06-30T23:59:60.5Z",
            "2027-01-01T12:00:00Z",
            "2027-01-01T23:59:00Z",
        ),
    ] {
        let parsed = Recurrence::parse(rule).expect("accepted");
        assert_eq!(
            parsed.next_after(at_utc(anchor), at_utc(after), 0, Timezone::UTC),
            Some(at_utc(next)),
            "{rule} anchored at {anchor}, asked after {after}"
        );
    }
}
