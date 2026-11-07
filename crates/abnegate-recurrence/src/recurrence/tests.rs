use super::*;

#[test]
fn each_accepted_clause_reads_back_as_itself() {
    let rule =
        Recurrence::parse("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=7;COUNT=4")
            .expect("the rule is accepted");

    assert_eq!(rule.frequency, Frequency::Weekly);
    assert_eq!(rule.interval, 2);
    assert_eq!(rule.by_day, vec![Weekday::Mon, Weekday::Wed]);
    assert_eq!(rule.by_hour, vec![9]);
    assert_eq!(rule.by_minute, vec![7]);
    assert_eq!(rule.count, Some(4));
    assert_eq!(rule.until, None);
}
