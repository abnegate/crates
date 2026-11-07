use abnegate_recurrence::Error;
use abnegate_recurrence::Timezone;
use chrono::DateTime;
use chrono::NaiveDateTime;
use chrono::Utc;

fn local(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").expect("the fixture is a time")
}

fn utc(text: &str) -> DateTime<Utc> {
    text.parse().expect("the fixture is an instant")
}

#[test]
fn a_name_the_tz_database_does_not_hold_is_refused_with_an_example() {
    let error = Timezone::parse("Middle/Earth").expect_err("no such timezone");
    assert!(
        matches!(error, Error::Timezone { ref name, .. } if name == "Middle/Earth"),
        "{error:?}"
    );
    let refused = error.to_string();
    assert!(refused.contains("Middle/Earth"), "{refused}");
    assert!(refused.contains("Europe/London"), "{refused}");
    assert_eq!(
        Timezone::parse(" Pacific/Auckland ").map(Timezone::name),
        Ok("Pacific/Auckland")
    );
}

#[test]
fn a_skipped_time_fires_at_the_instant_the_clocks_change() {
    let london = Timezone::parse("Europe/London").unwrap();
    assert_eq!(
        london.instant(local("2027-03-28 01:30")),
        Some(utc("2027-03-28T01:00:00Z"))
    );
    let auckland = Timezone::parse("Pacific/Auckland").unwrap();
    assert_eq!(
        auckland.instant(local("2027-09-26 02:30")),
        Some(utc("2027-09-25T14:00:00Z"))
    );
}

#[test]
fn a_time_shown_twice_fires_at_the_first_of_them() {
    let london = Timezone::parse("Europe/London").unwrap();
    assert_eq!(
        london.instant(local("2027-10-31 01:30")),
        Some(utc("2027-10-31T00:30:00Z"))
    );
    let auckland = Timezone::parse("Pacific/Auckland").unwrap();
    assert_eq!(
        auckland.instant(local("2027-04-04 02:30")),
        Some(utc("2027-04-03T13:30:00Z"))
    );
}

#[test]
fn a_skipped_time_with_no_minute_left_in_the_calendar_has_no_instant() {
    let new_york = Timezone::parse("America/New_York").unwrap();
    assert_eq!(new_york.instant(NaiveDateTime::MAX), None);
}
