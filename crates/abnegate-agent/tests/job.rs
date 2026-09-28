use abnegate_agent::tool::job::JobStarted;
use abnegate_agent::tool::job::parse_receipt;
use abnegate_agent::tool::job::started_text;

#[test]
fn a_job_built_outside_the_crate_round_trips_through_its_receipt() {
    let job = JobStarted::new(
        "job_9f3c1a7b2e04",
        48213,
        "/tmp/work/.abnegate/jobs/job_9f3c1a7b2e04.log",
    );

    assert_eq!(job.id, "job_9f3c1a7b2e04");
    assert_eq!(job.pid, 48213);
    assert_eq!(
        job.log_path,
        "/tmp/work/.abnegate/jobs/job_9f3c1a7b2e04.log"
    );
    assert_eq!(parse_receipt(&started_text(&job)), Some(job));
}
