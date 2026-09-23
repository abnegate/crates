use crate::catalog::error::CatalogError;
use crate::catalog::progress::DownloadProgress;
use futures::StreamExt;
use reqwest::Client;
use reqwest::StatusCode;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::io::BufWriter;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(3600);

/// Download a GGUF file, resuming a partial download when one is present.
///
/// Bytes land in a sibling `.part` file that is renamed onto `target` only once
/// the transfer completes, so an interrupted download never looks finished.
pub async fn download_gguf(
    url: &str,
    target: &Path,
    progress: Arc<DownloadProgress>,
) -> Result<(), CatalogError> {
    match transfer(url, target, &progress).await {
        Ok(()) => {
            progress.completed.store(true, Ordering::Relaxed);
            Ok(())
        }
        Err(error) => {
            progress.fail(error.to_string());
            Err(error)
        }
    }
}

async fn transfer(
    url: &str,
    target: &Path,
    progress: &DownloadProgress,
) -> Result<(), CatalogError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).await?;
    }

    let part = part_path(target);
    let existing = fs::metadata(&part)
        .await
        .map(|file| file.len())
        .unwrap_or(0);

    let client = Client::builder().timeout(DOWNLOAD_TIMEOUT).build()?;
    let mut request = client.get(url);
    if existing > 0 {
        request = request.header("Range", format!("bytes={existing}-"));
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() && status != StatusCode::PARTIAL_CONTENT {
        return Err(CatalogError::Unavailable(format!(
            "Download failed with status: {status}"
        )));
    }

    let resuming = status == StatusCode::PARTIAL_CONTENT;
    let content_length = response.content_length().unwrap_or(0);
    let total = if resuming {
        existing + content_length
    } else {
        content_length
    };
    progress.total_bytes.store(total, Ordering::Relaxed);

    let file = if resuming {
        progress.downloaded_bytes.store(existing, Ordering::Relaxed);
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part)
            .await?
    } else {
        progress.downloaded_bytes.store(0, Ordering::Relaxed);
        fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&part)
            .await?
    };

    let mut writer = BufWriter::new(file);
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        writer.write_all(&chunk).await?;
        progress
            .downloaded_bytes
            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }

    writer.flush().await?;
    drop(writer);

    fs::rename(&part, target).await?;
    Ok(())
}

fn part_path(target: &Path) -> std::path::PathBuf {
    target.with_extension(
        target
            .extension()
            .map(|extension| format!("{}.part", extension.to_string_lossy()))
            .unwrap_or_else(|| "part".to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;

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
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"gguf-body".to_vec()))
            .mount(&server)
            .await;

        let directory = tempdir().unwrap();
        let target = directory.path().join("nested").join("model.gguf");
        let progress = Arc::new(DownloadProgress::new());

        download_gguf(&server.uri(), &target, Arc::clone(&progress))
            .await
            .unwrap();

        assert_eq!(fs::read(&target).await.unwrap(), b"gguf-body");
        assert!(!target.with_extension("gguf.part").exists());
        assert!(progress.completed.load(Ordering::Relaxed));
        assert_eq!(progress.downloaded_bytes.load(Ordering::Relaxed), 9);
        assert_eq!(progress.percent(), 100);
    }

    #[tokio::test]
    async fn download_resumes_a_partial_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(b"-body".to_vec()))
            .mount(&server)
            .await;

        let directory = tempdir().unwrap();
        let target = directory.path().join("model.gguf");
        fs::write(target.with_extension("gguf.part"), b"gguf")
            .await
            .unwrap();
        let progress = Arc::new(DownloadProgress::new());

        download_gguf(&server.uri(), &target, Arc::clone(&progress))
            .await
            .unwrap();

        assert_eq!(fs::read(&target).await.unwrap(), b"gguf-body");
        assert_eq!(progress.total_bytes.load(Ordering::Relaxed), 9);
        assert_eq!(progress.downloaded_bytes.load(Ordering::Relaxed), 9);
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
        let progress = Arc::new(DownloadProgress::new());

        let error = download_gguf(&server.uri(), &target, Arc::clone(&progress))
            .await
            .unwrap_err();

        assert!(matches!(error, CatalogError::Unavailable(_)));
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
}
