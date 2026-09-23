use std::sync::Arc;
use std::sync::atomic::Ordering;

use sha2::Digest;
use sha2::Sha256;
use tempfile::tempdir;
use tokio::fs;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::header_exists;
use wiremock::matchers::method;

use super::*;

const BODY: &[u8] = b"gguf-body";
const ENTITY_TAG: &str = "\"v1\"";

fn checksum_of(bytes: &[u8]) -> Checksum {
    Checksum::new(Sha256::digest(bytes).into())
}

async fn partial(target: &Path, bytes: &[u8], entity_tag: Option<&str>) {
    let part = part_path(target);
    fs::write(&part, bytes).await.unwrap();
    if let Some(tag) = entity_tag {
        fs::write(
            Validator::path(&part),
            format!(
                r#"{{"entity_tag":{}}}"#,
                serde_json::to_string(tag).unwrap()
            ),
        )
        .await
        .unwrap();
    }
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
    partial(&target, b"gguf", Some(ENTITY_TAG)).await;

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
    partial(&target, b"stale", None).await;

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
    partial(&target, b"gguf", Some(ENTITY_TAG)).await;

    let (result, _) = download(&server, &target, None).await;
    result.unwrap();

    assert_eq!(fs::read(&target).await.unwrap(), b"new-model");
    let stored = Validator::load(&part_path(&target)).await;
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
    partial(&target, b"gguf", Some(ENTITY_TAG)).await;

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
    partial(&target, BODY, Some(ENTITY_TAG)).await;

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
    partial(&target, b"gguf-body-and-more", Some(ENTITY_TAG)).await;

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
    partial(&target, b"gguf", Some(ENTITY_TAG)).await;

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
