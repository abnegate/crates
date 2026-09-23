use std::io::Write as _;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use sha2::Digest;
use sha2::Sha256;
use tempfile::tempdir;
use tokio::fs;
use tokio::runtime::Runtime;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::header_exists;
use wiremock::matchers::method;

use super::*;

const BODY: &[u8] = b"gguf-body";
const ENTITY_TAG: &str = "\"v1\"";
const LARGE_BODY_BYTES: usize = 8 << 20;
const FIRST_CHUNK_BYTES: usize = 12 << 10;
const HEAD_END: &[u8] = b"\r\n\r\n";
const HELD_BACK: Duration = Duration::from_millis(200);
const POLL: Duration = Duration::from_millis(1);
const WAIT: Duration = Duration::from_secs(5);

fn checksum_of(bytes: &[u8]) -> Checksum {
    Checksum::new(Sha256::digest(bytes).into())
}

async fn partial(target: &Path, bytes: &[u8], entity_tag: Option<&str>, url: &str) {
    let part = part_path(target);
    fs::write(&part, bytes).await.unwrap();
    if let Some(tag) = entity_tag {
        validator(&part, tag, url).await;
    }
}

async fn validator(part: &Path, entity_tag: &str, url: &str) {
    fs::write(
        Validator::path(part),
        serde_json::json!({ "url": url, "entity_tag": entity_tag }).to_string(),
    )
    .await
    .unwrap();
}

async fn stored_guard(part: &Path, url: &str) -> Option<String> {
    Validator::load(part, url)
        .await
        .and_then(|validator| validator.if_range().map(str::to_string))
}

fn large_body() -> Arc<Vec<u8>> {
    Arc::new(
        (0..LARGE_BODY_BYTES)
            .map(|index| (index.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect(),
    )
}

/// Serves `body` from its own thread. The first request gets its head, then
/// nothing until `release` is signalled, then `sent` bytes and a closed
/// connection; every later request is answered in full. Reports each
/// request's resume offset.
fn flaky_server(
    body: Arc<Vec<u8>>,
    sent: usize,
) -> (String, mpsc::Receiver<Option<usize>>, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/model.gguf", listener.local_addr().unwrap());
    let (requests, offsets) = mpsc::channel();
    let (release, released) = mpsc::channel();
    std::thread::spawn(move || {
        let connections = listener.incoming().map_while(Result::ok).enumerate();
        for (index, mut socket) in connections {
            let offset = requested_offset(&mut socket);
            let _ = requests.send(offset);
            let length = body.len();
            let start = offset.unwrap_or(0);
            let status = match offset {
                Some(_) => format!(
                    "206 Partial Content\r\ncontent-range: bytes {start}-{}/{length}",
                    length - 1
                ),
                None => "200 OK".to_string(),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\netag: {ENTITY_TAG}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                length - start
            );
            let _ = socket.write_all(head.as_bytes());
            let end = if index == 0 {
                let _ = released.recv();
                start + sent
            } else {
                length
            };
            let _ = socket.write_all(&body[start..end]);
        }
    });
    (url, offsets, release)
}

fn requested_offset(socket: &mut TcpStream) -> Option<usize> {
    let mut head = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !head
        .windows(HEAD_END.len())
        .any(|window| window == HEAD_END)
    {
        let read = socket.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        head.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8_lossy(&head).lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.eq_ignore_ascii_case("range") {
            return None;
        }
        value
            .trim()
            .strip_prefix("bytes=")?
            .strip_suffix('-')?
            .parse()
            .ok()
    })
}

fn one_blocking_thread() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
}

/// Download from a [`flaky_server`] while the runtime's one blocking thread
/// is tied up for [`HELD_BACK`] from just before the failing body is
/// released, so any write the download merely started is still pending
/// when it returns.
async fn download_with_writes_held_back(
    url: &str,
    target: &Path,
    release: mpsc::Sender<()>,
) -> Result<(), DownloadError> {
    let validator = Validator::path(&part_path(target));
    let hold = async {
        tokio::time::timeout(WAIT, async {
            while !validator.exists() {
                tokio::time::sleep(POLL).await;
            }
        })
        .await
        .expect("the download stored its validator");
        drop(tokio::task::spawn_blocking(|| {
            std::thread::sleep(HELD_BACK)
        }));
        release.send(()).unwrap();
    };
    let progress = Arc::new(DownloadProgress::new());
    let (result, ()) = tokio::join!(download_gguf(url, target, None, progress), hold);
    result
}

/// Answer the resume of a four-byte `part` with the rest of [`BODY`], after
/// appending bytes the server never sends, as a writer outside the transfer
/// would.
async fn resume_with_stray_bytes(server: &MockServer, part: &Path) {
    let stray = part.to_path_buf();
    Mock::given(method("GET"))
        .and(header("range", "bytes=4-"))
        .respond_with(move |_: &Request| {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&stray)
                .and_then(|mut file| file.write_all(b"XX"))
                .unwrap();
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 4-8/9")
                .set_body_bytes(b"-body".to_vec())
        })
        .expect(1)
        .mount(server)
        .await;
}

fn part_length(part: &Path) -> u64 {
    std::fs::metadata(part).unwrap().len()
}

async fn download(
    server: &MockServer,
    target: &Path,
    expected: Option<Checksum>,
) -> (Result<(), DownloadError>, Arc<DownloadProgress>) {
    let progress = Arc::new(DownloadProgress::new());
    let result = download_gguf(&server.uri(), target, expected, Arc::clone(&progress)).await;
    (result, progress)
}

#[test]
fn part_path_appends_to_the_extension() {
    assert_eq!(
        part_path(Path::new("/models/model.gguf")),
        Path::new("/models/model.gguf.part")
    );
    assert_eq!(
        part_path(Path::new("/models/model")),
        Path::new("/models/model.part")
    );
}

#[tokio::test]
async fn download_writes_the_whole_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", ENTITY_TAG)
                .set_body_bytes(BODY.to_vec()),
        )
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("nested").join("model.gguf");

    let (result, progress) = download(&server, &target, Some(checksum_of(BODY))).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), BODY);
    assert!(!part_path(&target).exists());
    assert!(!Validator::path(&part_path(&target)).exists());
    assert!(!TransferLock::path(&part_path(&target)).exists());
    assert!(progress.completed.load(Ordering::Relaxed));
    assert_eq!(progress.downloaded_bytes.load(Ordering::Relaxed), 9);
    assert_eq!(progress.percent(), 100);
}

#[tokio::test]
async fn a_resume_is_guarded_by_the_stored_validator() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("range", "bytes=4-"))
        .and(header("if-range", ENTITY_TAG))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 4-8/9")
                .set_body_bytes(b"-body".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    partial(&target, b"gguf", Some(ENTITY_TAG), &server.uri()).await;

    let (result, progress) = download(&server, &target, Some(checksum_of(BODY))).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), BODY);
    assert_eq!(progress.total_bytes.load(Ordering::Relaxed), 9);
    assert_eq!(progress.downloaded_bytes.load(Ordering::Relaxed), 9);
}

#[tokio::test]
async fn a_part_with_no_validator_is_fetched_again_rather_than_resumed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header_exists("range"))
        .respond_with(ResponseTemplate::new(206).set_body_bytes(b"-body".to_vec()))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    partial(&target, b"stale", None, &server.uri()).await;

    let (result, _) = download(&server, &target, None).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), BODY);
}

#[tokio::test]
async fn a_changed_upstream_file_replaces_the_part_instead_of_being_spliced_onto_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("if-range", ENTITY_TAG))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v2\"")
                .set_body_bytes(b"new-model".to_vec()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(header_exists("range"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 4-8/9")
                .set_body_bytes(b"model".to_vec()),
        )
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    partial(&target, b"gguf", Some(ENTITY_TAG), &server.uri()).await;

    let (result, _) = download(&server, &target, None).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), b"new-model");
    let stored = Validator::load(&part_path(&target), &server.uri()).await;
    assert!(
        stored.is_none(),
        "a finished download leaves no validator behind"
    );
}

#[tokio::test]
async fn a_range_that_does_not_start_where_the_part_ends_is_discarded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header_exists("range"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 0-8/9")
                .set_body_bytes(BODY.to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    partial(&target, b"gguf", Some(ENTITY_TAG), &server.uri()).await;

    let (result, _) = download(&server, &target, None).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), BODY);
}

#[tokio::test]
async fn a_complete_part_answered_with_416_is_finished_rather_than_failed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(416))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-length", "9"))
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    partial(&target, BODY, Some(ENTITY_TAG), &server.uri()).await;

    let (result, progress) = download(&server, &target, Some(checksum_of(BODY))).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), BODY);
    assert_eq!(progress.percent(), 100);
}

#[tokio::test]
async fn a_416_for_a_part_of_the_wrong_length_starts_over() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header_exists("range"))
        .respond_with(ResponseTemplate::new(416))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-length", "9"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    partial(
        &target,
        b"gguf-body-and-more",
        Some(ENTITY_TAG),
        &server.uri(),
    )
    .await;

    let (result, _) = download(&server, &target, None).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), BODY);
}

#[tokio::test]
async fn a_file_that_does_not_match_its_checksum_is_refused_and_removed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"tampered!".to_vec()))
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");

    let (result, progress) = download(&server, &target, Some(checksum_of(BODY))).await;

    assert!(
        matches!(result, Err(DownloadError::Checksum { .. })),
        "{result:?}"
    );
    assert!(!target.exists());
    assert!(!part_path(&target).exists());
    assert!(progress.failed.load(Ordering::Relaxed));
}

#[tokio::test]
async fn a_short_body_is_kept_to_resume_rather_than_installed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 4-99/100")
                .set_body_bytes(b"-body".to_vec()),
        )
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    partial(&target, b"gguf", Some(ENTITY_TAG), &server.uri()).await;

    let (result, _) = download(&server, &target, None).await;

    assert!(
        matches!(
            result,
            Err(DownloadError::Incomplete {
                expected: 100,
                received: 9
            })
        ),
        "{result:?}"
    );
    assert!(!target.exists());
    assert_eq!(fs::read(part_path(&target)).await.unwrap(), BODY);
}

#[tokio::test]
async fn download_records_a_failing_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");

    let (result, progress) = download(&server, &target, None).await;

    assert!(matches!(result, Err(DownloadError::Status(404))));
    assert!(progress.failed.load(Ordering::Relaxed));
    assert!(!progress.completed.load(Ordering::Relaxed));
    assert!(
        progress
            .error_message
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|message| message.contains("404"))
    );
    assert!(!target.exists());
}

#[tokio::test]
async fn a_restart_that_cannot_open_the_part_keeps_it_paired_with_its_own_validator() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v2\"")
                .set_body_bytes(b"new-model".to_vec()),
        )
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    let part = part_path(&target);
    fs::create_dir(&part).await.unwrap();
    validator(&part, ENTITY_TAG, &server.uri()).await;

    let (result, _) = download(&server, &target, None).await;

    assert!(matches!(result, Err(DownloadError::Io(_))), "{result:?}");
    assert_eq!(
        stored_guard(&part, &server.uri()).await.as_deref(),
        Some(ENTITY_TAG),
        "the new file's validator was stored beside the old file's bytes"
    );
}

#[tokio::test]
async fn a_part_left_by_another_url_is_fetched_again_rather_than_resumed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header_exists("range"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 4-8/9")
                .set_body_bytes(b"-body".to_vec()),
        )
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    partial(
        &target,
        b"gguf",
        Some(ENTITY_TAG),
        "https://mirror.example/model.gguf",
    )
    .await;

    let (result, _) = download(&server, &target, None).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), BODY);
}

#[tokio::test]
async fn a_validator_that_names_no_url_is_not_trusted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header_exists("range"))
        .respond_with(ResponseTemplate::new(206).set_body_bytes(b"-body".to_vec()))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY.to_vec()))
        .mount(&server)
        .await;

    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    let part = part_path(&target);
    fs::write(&part, b"gguf").await.unwrap();
    fs::write(
        Validator::path(&part),
        serde_json::json!({ "entity_tag": ENTITY_TAG }).to_string(),
    )
    .await
    .unwrap();

    let (result, _) = download(&server, &target, None).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), BODY);
}

#[tokio::test]
async fn a_second_download_to_the_same_file_is_refused_while_the_first_runs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(BODY.to_vec())
                .set_delay(Duration::from_millis(300)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");

    let first = tokio::spawn({
        let url = server.uri();
        let target = target.clone();
        async move { download_gguf(&url, &target, None, Arc::new(DownloadProgress::new())).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first download reached the server");

    let (second, progress) = download(&server, &target, None).await;

    assert!(
        matches!(second, Err(DownloadError::InProgress)),
        "{second:?}"
    );
    assert!(progress.failed.load(Ordering::Relaxed));
    first.await.unwrap().unwrap();
    assert_eq!(fs::read(&target).await.unwrap(), BODY);
}

#[test]
fn a_failed_download_has_finished_writing_when_it_returns() {
    let (url, _, release) = flaky_server(large_body(), FIRST_CHUNK_BYTES);
    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    let part = part_path(&target);
    let runtime = one_blocking_thread();

    let result = runtime.block_on(download_with_writes_held_back(&url, &target, release));
    let returned = part_length(&part);
    drop(runtime);
    let settled = part_length(&part);

    assert!(matches!(result, Err(DownloadError::Http(_))), "{result:?}");
    assert_eq!(
        returned, settled,
        "a write landed on the part file after the download returned"
    );
    assert_eq!(returned, FIRST_CHUNK_BYTES as u64);
    assert!(!TransferLock::path(&part).exists());
}

#[test]
fn a_download_retried_straight_after_a_failure_resumes_where_the_part_ends() {
    let body = large_body();
    let (url, offsets, release) = flaky_server(Arc::clone(&body), FIRST_CHUNK_BYTES);
    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    let failing = one_blocking_thread();
    let retrying = one_blocking_thread();

    let first = failing.block_on(download_with_writes_held_back(&url, &target, release));
    let progress = Arc::new(DownloadProgress::new());
    let second = retrying.block_on(download_gguf(&url, &target, None, progress));
    drop(failing);

    assert!(matches!(first, Err(DownloadError::Http(_))), "{first:?}");
    second.unwrap();
    assert_eq!(
        offsets.try_iter().collect::<Vec<_>>(),
        [None, Some(FIRST_CHUNK_BYTES)],
        "the retry did not resume from the end of the part file"
    );
    assert!(
        std::fs::read(&target).unwrap() == *body,
        "the resumed file is not the upstream file"
    );
}

#[tokio::test]
async fn a_part_that_holds_more_than_was_received_is_not_installed() {
    let server = MockServer::start().await;
    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    let part = part_path(&target);
    resume_with_stray_bytes(&server, &part).await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    partial(&target, b"gguf", Some(ENTITY_TAG), &server.uri()).await;

    let (result, _) = download(&server, &target, None).await;

    assert!(
        matches!(result, Err(DownloadError::Status(503))),
        "{result:?}"
    );
    assert!(!target.exists());
    assert!(
        !part.exists(),
        "a part holding bytes the server never sent was kept to resume"
    );
    assert!(!Validator::path(&part).exists());
}

#[tokio::test]
async fn a_part_that_holds_more_than_was_received_is_fetched_again_from_the_first_byte() {
    let server = MockServer::start().await;
    let directory = tempdir().unwrap();
    let target = directory.path().join("model.gguf");
    let part = part_path(&target);
    resume_with_stray_bytes(&server, &part).await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", ENTITY_TAG)
                .set_body_bytes(BODY.to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    partial(&target, b"gguf", Some(ENTITY_TAG), &server.uri()).await;

    let (result, progress) = download(&server, &target, None).await;
    result.unwrap();

    let ranges: Vec<Option<String>> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| {
            request
                .headers
                .get("range")
                .map(|range| range.to_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(
        ranges,
        [Some("bytes=4-".to_owned()), None],
        "the download after the bad part did not start from the first byte"
    );
    assert_eq!(fs::read(&target).await.unwrap(), BODY);
    assert!(!part.exists());
    assert!(!Validator::path(&part).exists());
    assert_eq!(progress.downloaded_bytes.load(Ordering::Relaxed), 9);
    assert_eq!(progress.percent(), 100);
}
