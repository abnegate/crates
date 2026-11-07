use abnegate_recurrence::Clause;
use abnegate_recurrence::Error;
use abnegate_recurrence::Frequency;
use abnegate_recurrence::Recurrence;
use abnegate_recurrence::Timezone;

fn refusal(rule: &str) -> Error {
    Recurrence::parse(rule).expect_err(rule)
}

#[test]
fn every_refusal_reads_exactly_as_the_table_says() {
    for (rule, expected) in [
        ("", "An rrule cannot be empty. Give at least FREQ."),
        (
            "  RRULE:  ",
            "An rrule cannot be empty. Give at least FREQ.",
        ),
        ("FREQ=DAILY; BYHOUR", "\" BYHOUR\" is not NAME=VALUE."),
        (
            "FREQ=DAILY; bysetpos =-1",
            "BYSETPOS is not a clause this build accepts. Use FREQ, INTERVAL, BYDAY, BYHOUR, \
             BYMINUTE, BYMONTHDAY, UNTIL or COUNT.",
        ),
        (
            "FREQ= Secondly ",
            "FREQ=Secondly is not supported. Use HOURLY, DAILY, WEEKLY or MONTHLY; HOURLY is the \
             finest cadence a schedule can have.",
        ),
        (
            "FREQ=DAILY;INTERVAL=0",
            "INTERVAL=0 must be a positive whole number.",
        ),
        (
            "FREQ=DAILY;COUNT=-1",
            "COUNT=-1 must be a positive whole number.",
        ),
        (
            "FREQ=DAILY;UNTIL=tomorrow",
            "UNTIL=tomorrow must read as 20260914T090000Z.",
        ),
        (
            "FREQ=DAILY;BYHOUR=9, 24 ",
            "BYHOUR=24 is not a value BYHOUR takes.",
        ),
        ("FREQ=DAILY;BYHOUR= , ", "BYHOUR was given no values."),
        ("INTERVAL=2", "An rrule must state FREQ."),
        (
            "FREQ=DAILY;UNTIL=20260101T000000Z;COUNT=3",
            "Give UNTIL or COUNT, not both.",
        ),
        (
            "FREQ=DAILY;BYDAY=MO",
            "Daily does not use BYDAY. Drop the clause, or state the FREQ that reads it: BYDAY \
             needs WEEKLY, BYMONTHDAY needs MONTHLY, and BYHOUR and BYMINUTE need DAILY, WEEKLY \
             or MONTHLY.",
        ),
    ] {
        assert_eq!(refusal(rule).to_string(), expected, "{rule:?}");
    }

    assert_eq!(
        Timezone::parse(" Middle/Earth ")
            .expect_err("no such timezone")
            .to_string(),
        "\" Middle/Earth \" is not an IANA timezone name. Use one such as Europe/London or \
         Pacific/Auckland."
    );
}

#[test]
fn unread_clauses_are_named_in_a_fixed_order() {
    let error = refusal("FREQ=HOURLY;BYMINUTE=0;BYHOUR=9;BYDAY=MO");
    assert!(
        matches!(
            error,
            Error::Unread { frequency: Frequency::Hourly, ref clauses, .. }
                if clauses == &[Clause::Day, Clause::Hour, Clause::Minute]
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "Hourly does not use BYDAY, BYHOUR, BYMINUTE. Drop the clause, or state the FREQ that \
         reads it: BYDAY needs WEEKLY, BYMONTHDAY needs MONTHLY, and BYHOUR and BYMINUTE need \
         DAILY, WEEKLY or MONTHLY."
    );
}

#[test]
fn a_later_clause_of_the_same_name_replaces_the_earlier_one() {
    assert_eq!(
        Recurrence::parse("FREQ=DAILY;BYHOUR=9;BYHOUR=17"),
        Recurrence::parse("FREQ=DAILY;BYHOUR=17")
    );
    assert!(
        Recurrence::parse("FREQ=HOURLY;FREQ=DAILY;BYHOUR=9").is_ok(),
        "the later FREQ is the one that reads BYHOUR"
    );
}

#[test]
fn a_frequency_reads_as_its_refusals_name_it() {
    for (frequency, name) in [
        (Frequency::Hourly, "Hourly"),
        (Frequency::Daily, "Daily"),
        (Frequency::Weekly, "Weekly"),
        (Frequency::Monthly, "Monthly"),
    ] {
        assert_eq!(frequency.as_str(), name);
        assert_eq!(frequency.to_string(), name);
    }
}
