use super::*;
use crate::pull_request::IssueReference;
use crate::pull_request::PullRequestDetail;
use crate::pull_request::StateFilter;
use crate::pull_request::WorkflowRunFilter;
use std::collections::BTreeSet;
use std::env;
use std::fmt::Debug;
use std::num::NonZeroU32;

const TOKEN: &str = "ABNEGATE_VCS_PARITY_TOKEN";

const REPOSITORY: &str = "ABNEGATE_VCS_PARITY_REPOSITORY";

const NUMBERS: &str = "ABNEGATE_VCS_PARITY_NUMBERS";

const NEWEST_COMMENTS: u64 = 100;

const ORDER_ROWS: u8 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Same,
    SettledOnSecondRead,
    UnknownOnFirstRead,
    ReadAgainThroughRest,
    Different,
}

struct Row {
    number: u64,
    field: &'static str,
    evidence: String,
    rest: String,
    verdict: Verdict,
}

struct Reads<'a> {
    number: u64,
    first: &'a OpenPullRequest,
    second: &'a OpenPullRequest,
    rest: &'a RestReads,
}

struct RestReads {
    detail: PullRequestDetail,
    check_runs: Collected<CheckRun>,
    statuses: Collected<CommitStatus>,
    workflow_runs: Collected<WorkflowRun>,
    comments: Collected<IssueComment>,
    threads: ReviewThreadCount,
}

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

fn judged<T: PartialEq>(first: &T, second: &T, rest: &T) -> Verdict {
    if first == rest {
        Verdict::Same
    } else if second == rest {
        Verdict::SettledOnSecondRead
    } else {
        Verdict::Different
    }
}

fn row<T: PartialEq + Debug>(
    reads: &Reads<'_>,
    field: &'static str,
    read: fn(&OpenPullRequest) -> T,
    rest: T,
    summary: fn(&T) -> String,
) -> Row {
    let first = read(reads.first);
    let second = read(reads.second);
    let verdict = judged(&first, &second, &rest);
    let shown = match verdict {
        Verdict::SettledOnSecondRead => &second,
        _ => &first,
    };
    let (evidence, rest) = match verdict {
        Verdict::Different => (format!("{shown:?}"), format!("{rest:?}")),
        _ => (summary(shown), summary(&rest)),
    };
    Row {
        number: reads.number,
        field,
        evidence,
        rest,
        verdict,
    }
}

fn mergeability(reads: &Reads<'_>) -> Row {
    let first = reads.first.mergeable_state;
    let second = reads.second.mergeable_state;
    let rest = reads.rest.detail.mergeable_state;
    let verdict = if first == rest {
        Verdict::Same
    } else if (first == MergeableState::Unknown && second == rest)
        || (rest == MergeableState::Unknown && first == second)
    {
        Verdict::UnknownOnFirstRead
    } else {
        Verdict::Different
    };
    Row {
        number: reads.number,
        field: "mergeable state",
        evidence: format!("{first:?} then {second:?}"),
        rest: format!("{rest:?}"),
        verdict,
    }
}

fn check_runs(collected: &Collected<CheckRun>) -> Vec<(Option<u64>, String, Option<String>)> {
    let mut runs: Vec<(Option<u64>, String, Option<String>)> = collected
        .items
        .iter()
        .map(|run| (run.id, run.status.clone(), run.conclusion.clone()))
        .collect();
    runs.sort();
    runs
}

fn statuses(collected: &Collected<CommitStatus>) -> Vec<(String, String)> {
    let mut statuses: Vec<(String, String)> = collected
        .items
        .iter()
        .map(|status| (status.context.clone(), status.state.clone()))
        .collect();
    statuses.sort();
    statuses
}

fn workflow_runs(
    collected: &Collected<WorkflowRun>,
) -> BTreeSet<(u64, Option<String>, Option<String>)> {
    collected
        .items
        .iter()
        .map(|run| (run.id.get(), run.status.clone(), run.conclusion.clone()))
        .collect()
}

fn comments(collected: &Collected<IssueComment>) -> (Vec<(u64, String)>, bool) {
    (
        collected
            .items
            .iter()
            .map(|comment| (comment.id, comment.author.clone()))
            .collect(),
        collected.complete,
    )
}

fn head(read: &OpenPullRequest) -> HeadChecks {
    read.head.clone().expect("a head read whole")
}

fn counted<T>(items: &[T]) -> String {
    format!("{} rows", items.len())
}

fn head_rows(reads: &Reads<'_>) -> Vec<Row> {
    let rest = reads.rest;
    let complete = |read: &OpenPullRequest| {
        read.head.as_ref().is_some_and(|head| {
            head.check_runs.complete && head.workflow_runs.complete && head.statuses.complete
        })
    };
    if !complete(reads.first) && !complete(reads.second) {
        return vec![Row {
            number: reads.number,
            field: "head checks",
            evidence: "head moved or a list was cut short".to_string(),
            rest: "read again through REST".to_string(),
            verdict: Verdict::ReadAgainThroughRest,
        }];
    }
    let reads = Reads {
        first: match complete(reads.first) {
            true => reads.first,
            false => reads.second,
        },
        ..*reads
    };
    vec![
        row(
            &reads,
            "check runs (id, status, conclusion)",
            |read| check_runs(&head(read).check_runs),
            check_runs(&rest.check_runs),
            |runs| counted(runs),
        ),
        row(
            &reads,
            "statuses (context, state)",
            |read| statuses(&head(read).statuses),
            statuses(&rest.statuses),
            |statuses| counted(statuses),
        ),
        row(
            &reads,
            "workflow runs (id, status, conclusion)",
            |read| workflow_runs(&head(read).workflow_runs),
            workflow_runs(&rest.workflow_runs),
            |runs| format!("{} rows", runs.len()),
        ),
    ]
}

fn compared(reads: &Reads<'_>) -> Vec<Row> {
    let rest = reads.rest;
    let mut rows = vec![
        row(
            reads,
            "draft",
            |read| read.listed.draft,
            rest.detail.draft,
            |draft| draft.to_string(),
        ),
        mergeability(reads),
        row(
            reads,
            "author",
            |read| read.listed.author_login.clone(),
            rest.detail.author_login.clone(),
            |author| author.clone().unwrap_or_default(),
        ),
        row(
            reads,
            "head sha",
            |read| read.listed.head_sha.clone(),
            Some(rest.detail.head_sha.clone()),
            |sha| {
                sha.as_ref()
                    .map(|sha| sha.as_str()[..12].to_string())
                    .unwrap_or_default()
            },
        ),
        row(
            reads,
            "comments (id, author), complete",
            |read| comments(&read.comments),
            comments(&rest.comments),
            |(comments, complete)| format!("{} rows, complete {complete}", comments.len()),
        ),
        row(
            reads,
            "threads (open, resolved, complete)",
            |read| read.threads,
            rest.threads,
            |threads| format!("{} {} {}", threads.open, threads.resolved, threads.complete),
        ),
    ];
    rows.extend(head_rows(reads));
    rows
}

async fn newest_comments(
    service: &PullRequestService,
    token: &SecretValue,
    reference: &IssueReference,
) -> Collected<IssueComment> {
    let counted = service
        .fetch_issue(reference, token)
        .await
        .unwrap()
        .comments;
    let last_page = counted.div_ceil(NEWEST_COMMENTS).max(1);
    let mut read: Vec<IssueComment> = Vec::new();
    for page in last_page.saturating_sub(1).max(1)..=last_page {
        let page = NonZeroU32::new(u32::try_from(page).unwrap()).unwrap();
        read.extend(
            service
                .list_comments(reference, token, page, NonZeroU8::new(100).unwrap())
                .await
                .unwrap()
                .items,
        );
    }
    let keep = read
        .len()
        .saturating_sub(usize::try_from(NEWEST_COMMENTS).unwrap());
    Collected::new(read.split_off(keep), counted <= NEWEST_COMMENTS)
}

async fn rest_reads(
    service: &PullRequestService,
    token: &SecretValue,
    repository: &Repository,
    number: NonZeroU64,
) -> RestReads {
    let reference = PullRequestReference::new(repository.clone(), number);
    let detail = service.fetch_pull(&reference, token).await.unwrap();
    let sha = detail.head_sha.clone();
    RestReads {
        check_runs: service
            .collect_check_runs(repository, token, &sha)
            .await
            .unwrap(),
        statuses: service
            .collect_statuses(repository, token, &sha)
            .await
            .unwrap(),
        workflow_runs: service
            .collect_workflow_runs(
                repository,
                token,
                &WorkflowRunFilter::new().with_head_sha(sha),
            )
            .await
            .unwrap(),
        comments: newest_comments(
            service,
            token,
            &IssueReference::new(repository.clone(), number),
        )
        .await,
        threads: service
            .fetch_review_thread_counts(&reference, token)
            .await
            .unwrap(),
        detail,
    }
}

async fn order_row(
    service: &PullRequestService,
    token: &SecretValue,
    repository: &Repository,
) -> String {
    let rows = NonZeroU8::new(ORDER_ROWS).unwrap();
    let open = service
        .fetch_open_pull_requests(repository, token, rows, None)
        .await
        .unwrap();
    let rest = service
        .list_pull_requests(repository, token, StateFilter::Open, NonZeroU32::MIN, rows)
        .await
        .unwrap();
    let numbers = |listed: &[ListedPullRequest]| -> Vec<u64> {
        listed
            .iter()
            .map(|row| row.reference.number().get())
            .collect()
    };
    format!(
        "open order: graphql {:?}, rest {:?}, same {}",
        numbers(&open.pull_requests),
        numbers(&rest.items),
        numbers(&open.pull_requests) == numbers(&rest.items)
    )
}

#[tokio::test]
#[ignore = "reads a live repository on GitHub: set ABNEGATE_VCS_PARITY_TOKEN, ABNEGATE_VCS_PARITY_REPOSITORY and ABNEGATE_VCS_PARITY_NUMBERS"]
async fn batched_evidence_reads_what_rest_reads() {
    let token = SecretValue::new(required(TOKEN));
    let named = required(REPOSITORY);
    let (owner, name) = named.split_once('/').expect("owner/name");
    let numbers: Vec<NonZeroU64> = required(NUMBERS)
        .split(',')
        .map(|number| number.trim().parse().expect("a pull request number"))
        .collect();
    let service = PullRequestService::new().unwrap();
    let repository = Repository::new(owner, name).unwrap();

    let order = order_row(&service, &token, &repository).await;
    let repository_id = service
        .fetch_open_pull_requests(&repository, &token, NonZeroU8::MIN, None)
        .await
        .unwrap()
        .repository_id;
    let mut ids: Vec<String> = Vec::new();
    for number in &numbers {
        let reference = PullRequestReference::new(repository.clone(), *number);
        ids.push(
            service
                .fetch_pull(&reference, &token)
                .await
                .unwrap()
                .node_id,
        );
    }

    let first = service
        .fetch_pull_request_evidence(&repository, &token, &repository_id, &ids)
        .await
        .unwrap();
    let mut rest: Vec<RestReads> = Vec::new();
    for number in &numbers {
        rest.push(rest_reads(&service, &token, &repository, *number).await);
    }
    let second = service
        .fetch_pull_request_evidence(&repository, &token, &repository_id, &ids)
        .await
        .unwrap();

    let mut rows: Vec<Row> = Vec::new();
    for (index, number) in numbers.iter().enumerate() {
        let (Some(first), Some(second)) = (&first[index], &second[index]) else {
            rows.push(Row {
                number: number.get(),
                field: "evidence",
                evidence: "none".to_string(),
                rest: "read".to_string(),
                verdict: Verdict::Different,
            });
            continue;
        };
        rows.extend(compared(&Reads {
            number: number.get(),
            first,
            second,
            rest: &rest[index],
        }));
    }

    println!("{order}");
    println!("| PR | field | EVIDENCE | REST | verdict |");
    println!("|---|---|---|---|---|");
    for row in &rows {
        println!(
            "| {} | {} | {} | {} | {:?} |",
            row.number, row.field, row.evidence, row.rest, row.verdict
        );
    }
    let different: Vec<&Row> = rows
        .iter()
        .filter(|row| row.verdict == Verdict::Different)
        .collect();
    assert!(
        different.is_empty(),
        "{} unexplained differences",
        different.len()
    );
}
